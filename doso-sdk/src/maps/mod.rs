mod grid;
mod point_cloud;

use crate::{
    common::text, rpc, CommandReceipt, DosoClient, Error, MapOperation, MapState, Pose,
    ProductOperation, ProductState, Result, Terminal,
};
use std::time::Duration;
pub struct Maps<'a>(pub(crate) &'a DosoClient);
impl Maps<'_> {
    pub async fn list(&self) -> Result<rpc::MapList> {
        DosoClient::read(
            rpc::maps_client::MapsClient::new(self.0.channel.clone())
                .list(self.0.request(rpc::Empty {}))
                .await,
        )
    }
    pub async fn save(
        &self,
        name: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<CommandReceipt> {
        if let Some(name) = name {
            text(name, "name")?;
        }
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let receipt = DosoClient::receipt(
            rpc::maps_client::MapsClient::new(self.0.channel.clone())
                .save(self.0.request(rpc::SaveMap {
                    command: Some(command),
                    name: name.map(str::to_owned),
                }))
                .await,
            &id,
        )?;
        if receipt.accepted == Some(true)
            && receipt.operation_id.as_deref().is_none_or(str::is_empty)
        {
            return Err(Error::protocol("accepted map save requires operation_id").request(&id));
        }
        Ok(receipt)
    }
    pub async fn operation(&self, operation_id: &str) -> Result<MapOperation> {
        text(operation_id, "operation_id")?;
        let operation = DosoClient::read(
            rpc::maps_client::MapsClient::new(self.0.channel.clone())
                .operation(self.0.request(rpc::MapQuery {
                    operation_id: operation_id.into(),
                }))
                .await,
        )?;
        if operation.operation_id != operation_id {
            return Err(Error::protocol("map operation identity mismatch"));
        }
        Ok(operation)
    }
    /// Wait for the existing operation; never resubmit its write.
    pub async fn wait(
        &self,
        operation_id: &str,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<MapOperation> {
        wait_config(timeout, poll_interval)?;
        tokio::time::timeout(timeout, async {
            loop {
                let operation = self.operation(operation_id).await?;
                match MapState::try_from(operation.state) {
                    Ok(MapState::MapSucceeded) => return Ok(operation),
                    Ok(MapState::MapFailed | MapState::MapCancelled) => {
                        return Err(Error::failed(Terminal::Map(operation)))
                    }
                    Ok(
                        MapState::MapUnknown
                        | MapState::MapWaitingSnapshot
                        | MapState::MapQueued
                        | MapState::MapRunning,
                    ) => {}
                    _ => return Err(Error::protocol("unknown map operation state")),
                }
                tokio::time::sleep(poll_interval).await;
            }
        })
        .await
        .map_err(|_| Error::timeout())?
    }
    pub async fn load(
        &self,
        map_name: &str,
        expected_product_session_id: &str,
        initial_pose: Option<Pose>,
        request_id: Option<&str>,
    ) -> Result<ProductOperation> {
        text(map_name, "map_name")?;
        if let Some(pose) = &initial_pose {
            pose.validate()?;
        }
        self.switch(
            rpc::Product::Nav,
            expected_product_session_id,
            Some(map_name),
            initial_pose,
            request_id,
        )
        .await
    }
    pub async fn start_mapping(
        &self,
        expected_product_session_id: &str,
        request_id: Option<&str>,
    ) -> Result<ProductOperation> {
        self.switch(
            rpc::Product::Map,
            expected_product_session_id,
            None,
            None,
            request_id,
        )
        .await
    }
    async fn switch(
        &self,
        product: rpc::Product,
        expected: &str,
        map_name: Option<&str>,
        initial_pose: Option<Pose>,
        request_id: Option<&str>,
    ) -> Result<ProductOperation> {
        let command = self.0.command(request_id)?;
        let id = command.request_id.clone();
        let operation = rpc::maps_client::MapsClient::new(self.0.channel.clone())
            .switch(self.0.request(rpc::SwitchProduct {
                command: Some(command),
                product: product as i32,
                expected_product_session_id: expected.into(),
                map_name: map_name.map(str::to_owned),
                initial_pose,
            }))
            .await
            .map_err(|e| Error::rpc(e, Some(&id)))?
            .into_inner();
        if operation.request_id != id {
            return Err(Error::protocol("product switch identity mismatch").request(&id));
        }
        Ok(operation)
    }
    pub async fn product_operation(&self, request_id: &str) -> Result<ProductOperation> {
        text(request_id, "request_id")?;
        let operation = DosoClient::read(
            rpc::maps_client::MapsClient::new(self.0.channel.clone())
                .product_operation(self.0.request(rpc::ProductQuery {
                    request_id: request_id.into(),
                }))
                .await,
        )?;
        if operation.request_id != request_id {
            return Err(Error::protocol("product operation identity mismatch"));
        }
        Ok(operation)
    }
    pub async fn wait_product_operation(
        &self,
        request_id: &str,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<ProductOperation> {
        wait_config(timeout, poll_interval)?;
        tokio::time::timeout(timeout, async {
            loop {
                let operation = self.product_operation(request_id).await?;
                match ProductState::try_from(operation.state) {
                    Ok(ProductState::ProductSucceeded) => {
                        if !operation.active {
                            return Err(Error::protocol("product success requires active result"));
                        }
                        return Ok(operation);
                    }
                    Ok(ProductState::ProductFailed | ProductState::ProductInterrupted) => {
                        return Err(Error::failed(Terminal::Product(operation)))
                    }
                    Ok(ProductState::Unknown | ProductState::ProductRunning) => {}
                    _ => return Err(Error::protocol("unknown product operation state")),
                }
                tokio::time::sleep(poll_interval).await;
            }
        })
        .await
        .map_err(|_| Error::timeout())?
    }
}
fn wait_config(timeout: Duration, poll_interval: Duration) -> Result<()> {
    if timeout.is_zero() || poll_interval.is_zero() {
        return Err(Error::invalid("timeout and poll interval must be positive"));
    }
    Ok(())
}
