use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ControlOwner {
    None = 0,
    Grpc = 1,
    Yunzhuo = 2,
}

#[derive(Debug, Clone)]
pub struct ControlArbiter {
    owner: ControlOwner,
    owner_id: String,
    lease_expires_us: u64,
    last_sequence: u64,
    accepted_count: u64,
    rejected_count: u64,
    lease_timeout_us: u64,
}

impl ControlArbiter {
    pub fn new(lease_timeout_ms: u64) -> Self {
        Self {
            owner: ControlOwner::None,
            owner_id: String::new(),
            lease_expires_us: 0,
            last_sequence: 0,
            accepted_count: 0,
            rejected_count: 0,
            lease_timeout_us: lease_timeout_ms.saturating_mul(1000),
        }
    }

    pub fn owner(&self) -> ControlOwner {
        self.owner
    }

    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    pub fn grpc_lease_active(&self, now_us: u64) -> bool {
        self.owner == ControlOwner::Grpc && now_us < self.lease_expires_us
    }

    pub fn lease_remaining_ms(&self, now_us: u64) -> u32 {
        if !self.grpc_lease_active(now_us) {
            return 0;
        }
        ((self.lease_expires_us - now_us) / 1000) as u32
    }

    pub fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub fn counts(&self) -> (u64, u64) {
        (self.accepted_count, self.rejected_count)
    }

    /// Physical remote (`YUNZHUO`) unconditionally preempts any software gRPC lease.
    pub fn preempt_yunzhuo(&mut self, now_us: u64) {
        self.owner = ControlOwner::Yunzhuo;
        self.owner_id = "yunzhuo@hw".to_string();
        self.lease_expires_us = now_us.saturating_add(self.lease_timeout_us);
    }

    pub fn release_yunzhuo(&mut self) {
        if self.owner == ControlOwner::Yunzhuo {
            self.owner = ControlOwner::None;
            self.owner_id.clear();
            self.lease_expires_us = 0;
        }
    }

    pub fn acquire_grpc(&mut self, caller_id: &str, now_us: u64) -> Result<(), &'static str> {
        self.expire_if_needed(now_us);
        if self.owner == ControlOwner::Yunzhuo {
            self.rejected_count += 1;
            return Err("preempted_by_yunzhuo");
        }
        if self.owner == ControlOwner::Grpc && self.owner_id != caller_id {
            self.rejected_count += 1;
            return Err("grpc_lease_held_by_another_principal");
        }
        self.owner = ControlOwner::Grpc;
        self.owner_id = caller_id.to_string();
        self.lease_expires_us = now_us.saturating_add(self.lease_timeout_us);
        self.last_sequence = 0;
        self.accepted_count += 1;
        Ok(())
    }

    pub fn renew_grpc_seq(
        &mut self,
        caller_id: &str,
        seq: u64,
        now_us: u64,
    ) -> Result<(), &'static str> {
        self.expire_if_needed(now_us);
        if self.owner != ControlOwner::Grpc || self.owner_id != caller_id {
            self.rejected_count += 1;
            return Err("lease_missing");
        }
        if seq == 0 || seq != self.last_sequence + 1 {
            self.rejected_count += 1;
            return Err("sequence_violation");
        }
        self.last_sequence = seq;
        self.lease_expires_us = now_us.saturating_add(self.lease_timeout_us);
        self.accepted_count += 1;
        Ok(())
    }

    pub fn release_grpc(&mut self, caller_id: &str) {
        if self.owner == ControlOwner::Grpc && (caller_id.is_empty() || self.owner_id == caller_id)
        {
            self.owner = ControlOwner::None;
            self.owner_id.clear();
            self.lease_expires_us = 0;
            self.last_sequence = 0;
        }
    }

    pub fn force_revoke(&mut self) {
        if self.owner == ControlOwner::Grpc {
            self.owner = ControlOwner::None;
            self.owner_id.clear();
            self.lease_expires_us = 0;
            self.last_sequence = 0;
        }
    }

    pub fn expire_if_needed(&mut self, now_us: u64) -> bool {
        if self.owner != ControlOwner::None && now_us >= self.lease_expires_us {
            self.owner = ControlOwner::None;
            self.owner_id.clear();
            self.lease_expires_us = 0;
            self.last_sequence = 0;
            return true;
        }
        false
    }
}
