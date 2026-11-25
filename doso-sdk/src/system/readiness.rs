use std::collections::BTreeMap;

use prost_types::{value::Kind, Value};

use crate::{rpc, Error, Result};

use super::System;

impl System<'_> {
    /// Read the service's readiness fields without assuming missing values are false.
    pub async fn readiness_state(&self) -> Result<rpc::ReadinessState> {
        project(self.readiness().await?)
    }
}

fn project(snapshot: rpc::Snapshot) -> Result<rpc::ReadinessState> {
    let fields = snapshot
        .fields
        .ok_or_else(|| Error::protocol("readiness snapshot has no fields"))?
        .fields;
    let status = match fields.get("status").and_then(|value| value.kind.as_ref()) {
        Some(Kind::StringValue(value)) => value.clone(),
        None | Some(Kind::NullValue(_)) => String::new(),
        _ => return Err(Error::protocol("readiness status must be a string")),
    };
    Ok(rpc::ReadinessState {
        status,
        ready: optional_bool(&fields, "ready")?,
        data_ready: optional_bool(&fields, "data_ready")?,
        motion_ready: optional_bool(&fields, "motion_ready")?,
        non_motion_safe: optional_bool(&fields, "non_motion_safe")?,
        reasons: strings(&fields, "reasons")?,
        advisories: strings(&fields, "advisories")?,
    })
}

fn optional_bool(fields: &BTreeMap<String, Value>, name: &str) -> Result<Option<bool>> {
    match fields.get(name).and_then(|value| value.kind.as_ref()) {
        Some(Kind::BoolValue(value)) => Ok(Some(*value)),
        None | Some(Kind::NullValue(_)) => Ok(None),
        _ => Err(Error::protocol(format!(
            "readiness {name} must be a boolean"
        ))),
    }
}

fn strings(fields: &BTreeMap<String, Value>, name: &str) -> Result<Vec<String>> {
    match fields.get(name).and_then(|value| value.kind.as_ref()) {
        None | Some(Kind::NullValue(_)) => Ok(Vec::new()),
        Some(Kind::ListValue(list)) => list
            .values
            .iter()
            .map(|value| match value.kind.as_ref() {
                Some(Kind::StringValue(value)) => Ok(value.clone()),
                _ => Err(Error::protocol(format!(
                    "readiness {name} must contain strings"
                ))),
            })
            .collect(),
        _ => Err(Error::protocol(format!("readiness {name} must be a list"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost_types::{ListValue, Struct};

    fn value(kind: Kind) -> Value {
        Value { kind: Some(kind) }
    }

    #[test]
    fn preserves_readiness_and_unknown_fields() {
        let fields = BTreeMap::from([
            ("status".into(), value(Kind::StringValue("degraded".into()))),
            ("ready".into(), value(Kind::BoolValue(false))),
            ("data_ready".into(), value(Kind::BoolValue(true))),
            (
                "reasons".into(),
                value(Kind::ListValue(ListValue {
                    values: vec![value(Kind::StringValue("localization:lost".into()))],
                })),
            ),
        ]);
        let state = project(rpc::Snapshot {
            fields: Some(Struct { fields }),
        })
        .unwrap();
        assert_eq!(state.status, "degraded");
        assert_eq!(state.ready, Some(false));
        assert_eq!(state.data_ready, Some(true));
        assert_eq!(state.motion_ready, None);
        assert_eq!(state.reasons, ["localization:lost"]);
    }

    #[test]
    fn rejects_invalid_fields() {
        let fields = BTreeMap::from([("ready".into(), value(Kind::StringValue("yes".into())))]);
        assert!(project(rpc::Snapshot {
            fields: Some(Struct { fields })
        })
        .is_err());
        assert!(project(rpc::Snapshot { fields: None }).is_err());
    }
}
