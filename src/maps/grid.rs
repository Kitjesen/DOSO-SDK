use crate::{common::text, rpc, DosoClient, Error, Result};

use super::Maps;

const MAX_CELLS: usize = 25_000_000;

impl Maps<'_> {
    /// Read a saved occupancy grid. Unknown cells are 255; rows increase along +y.
    pub async fn grid(&self, map_name: &str) -> Result<rpc::MapGrid> {
        text(map_name, "map_name")?;
        let grid = DosoClient::read(
            rpc::maps_client::MapsClient::new(self.0.channel.clone())
                .max_decoding_message_size(MAX_CELLS + 64 * 1024)
                .grid(self.0.request(rpc::MapContentRequest {
                    map_name: map_name.into(),
                }))
                .await,
        )?;
        if grid.map_id != map_name || grid.frame_id.is_empty() {
            return Err(Error::protocol("map grid identity is invalid"));
        }
        let cells = (grid.width as usize).checked_mul(grid.height as usize);
        if cells.is_none_or(|count| count == 0 || count > MAX_CELLS || count != grid.cells.len())
            || !grid.resolution_m.is_finite()
            || grid.resolution_m <= 0.0
            || grid.cells.iter().any(|value| *value > 100 && *value != 255)
        {
            return Err(Error::protocol("map grid geometry or cells are invalid"));
        }
        grid.origin
            .as_ref()
            .ok_or_else(|| Error::protocol("map grid origin is missing"))?
            .validate()
            .map_err(|_| Error::protocol("map grid origin is invalid"))?;
        Ok(grid)
    }
}
