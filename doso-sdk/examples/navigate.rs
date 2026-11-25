use doso_sdk::{new_request_id, Config, DosoClient, ErrorKind, Pose, TaskState};
use std::time::Duration;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Args {
    endpoint: String,
    pose: Pose,
    timeout: Duration,
}

impl Args {
    fn parse(values: &[String]) -> Result<Self> {
        if !matches!(values.len(), 5 | 6) {
            return Err("Expected endpoint, x, y, z, yaw and optional timeout; use --help".into());
        }
        let seconds: u64 = values.get(5).map_or(Ok(120), |value| value.parse())?;
        if seconds == 0 {
            return Err("Timeout must be positive whole seconds".into());
        }
        Ok(Self {
            endpoint: values[0].clone(),
            pose: Pose {
                x: values[1].parse()?,
                y: values[2].parse()?,
                z: values[3].parse()?,
                yaw: values[4].parse()?,
            },
            timeout: Duration::from_secs(seconds),
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let values: Vec<String> = std::env::args().skip(1).collect();
    if values.len() == 1 && matches!(values[0].as_str(), "--help" | "-h") {
        println!("Submit one map-frame navigation goal and wait for its result:");
        println!("navigate <http(s)://robot:5052> <x> <y> <z> <yaw> [timeout_seconds]");
        println!("XYZ: metres; yaw: radians; local wait timeout: 120 seconds by default.");
        println!("This command sends a navigation goal. A local timeout does not cancel it.");
        println!("DOSO_API_KEY: optional API key; DOSO_TLS_CA_FILE: optional HTTPS PEM CA.");
        return Ok(());
    }
    let args = Args::parse(&values)?;
    let mut config = Config::new(args.endpoint);
    config.api_key = std::env::var("DOSO_API_KEY").ok();
    if let Some(path) = std::env::var_os("DOSO_TLS_CA_FILE") {
        config.tls_ca_certificate = Some(std::fs::read(path)?);
    }
    let robot = DosoClient::with_config(config).await?;
    let navigation = robot.navigation();
    let request_id = new_request_id()?;
    println!("request_id={request_id}");
    let receipt = match navigation.go_to(args.pose, Some(&request_id)).await {
        Ok(receipt) => receipt,
        Err(error) => {
            eprintln!(
                "submission error: request_id={request_id} submission_unknown={} code={:?} reason_code={:?}",
                error.submission_unknown, error.code(), error.reason_code()
            );
            return Err(error.into());
        }
    };
    println!(
        "admission accepted={:?} task_id={:?} reason_code={} reason={}",
        receipt.accepted, receipt.task_id, receipt.reason_code, receipt.reason
    );
    match receipt.accepted {
        Some(true) => {}
        Some(false) => return Err("Navigation goal was rejected; keep the request_id".into()),
        None => {
            return Err(
                "Navigation admission is unknown; inspect the request before retrying".into(),
            )
        }
    }
    let task_id = receipt
        .task_id
        .as_deref()
        .ok_or("Missing navigation task_id")?;
    println!("task_id={task_id}");
    match navigation.wait(task_id, args.timeout).await {
        Ok(task) => println!(
            "task_id={} state={:?} reason={}",
            task.task_id,
            TaskState::try_from(task.state),
            task.reason
        ),
        Err(error) => {
            eprintln!("wait error: request_id={request_id} task_id={task_id}: {error}");
            if error.kind == ErrorKind::Timeout {
                eprintln!("The remote task was not cancelled; query task_id={task_id} before resubmitting.");
            }
            return Err(error.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(arguments: &str) -> Vec<String> {
        arguments.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn parses_map_goal_and_default_timeout() {
        let args = Args::parse(&values("http://localhost:5052 3 -2 0.4 0.7")).unwrap();
        assert_eq!(args.endpoint, "http://localhost:5052");
        assert_eq!(
            args.pose,
            Pose {
                x: 3.0,
                y: -2.0,
                z: 0.4,
                yaw: 0.7
            }
        );
        assert_eq!(args.timeout, Duration::from_secs(120));
    }

    #[test]
    fn parses_explicit_timeout() {
        let args = Args::parse(&values("http://localhost:5052 3 2 0 0 30")).unwrap();
        assert_eq!(args.timeout, Duration::from_secs(30));
    }

    #[test]
    fn rejects_bad_arguments_before_connecting() {
        for arguments in [
            "http://localhost:5052 3 2 0",
            "http://localhost:5052 bad 2 0 0",
            "http://localhost:5052 3 2 0 0 0",
            "http://localhost:5052 3 2 0 0 -1",
            "http://localhost:5052 3 2 0 0 30 extra",
        ] {
            assert!(Args::parse(&values(arguments)).is_err(), "{arguments}");
        }
    }
}
