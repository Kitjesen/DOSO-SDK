use crate::rpc;
use prost::Message;
use prost_types::Struct;
use std::fmt;
use tonic::Code;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidInput,
    Transport,
    Rpc(Code),
    Protocol,
    OperationFailed,
    Timeout,
}

#[derive(Debug)]
pub enum Terminal {
    Navigation(rpc::Task),
    Map(rpc::MapOperation),
    Product(rpc::ProductOperation),
}

#[derive(Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub request_id: Option<String>,
    pub submission_unknown: bool,
    pub terminal: Option<Box<Terminal>>,
    detail: Option<Box<rpc::ErrorDetail>>,
}
impl Error {
    pub fn code(&self) -> Code {
        match &self.kind {
            ErrorKind::InvalidInput => Code::InvalidArgument,
            ErrorKind::Transport => Code::Unavailable,
            ErrorKind::Rpc(code) => *code,
            ErrorKind::Protocol => Code::Internal,
            ErrorKind::OperationFailed => Code::Unknown,
            ErrorKind::Timeout => Code::DeadlineExceeded,
        }
    }
    pub fn reason_code(&self) -> Option<&str> {
        let reason = match self.terminal.as_deref() {
            Some(Terminal::Navigation(task)) => &task.reason_code,
            Some(Terminal::Map(operation)) => &operation.reason_code,
            Some(Terminal::Product(operation)) => &operation.reason_code,
            None => &self.detail.as_ref()?.reason_code,
        };
        (!reason.is_empty()).then_some(reason.as_str())
    }
    pub fn details(&self) -> Option<&Struct> {
        match self.terminal.as_deref() {
            Some(Terminal::Navigation(task)) => task.details.as_ref(),
            Some(Terminal::Map(operation)) => operation.details.as_ref(),
            Some(Terminal::Product(operation)) => operation.details.as_ref(),
            None => self.detail.as_ref()?.details.as_ref(),
        }
    }
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            request_id: None,
            submission_unknown: false,
            terminal: None,
            detail: None,
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }
    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Protocol, message)
    }
    pub(crate) fn failed(terminal: Terminal) -> Self {
        let message = match &terminal {
            Terminal::Navigation(task) => format!("navigation ended: {}", task.reason),
            Terminal::Map(operation) => format!("map operation ended: {}", operation.reason),
            Terminal::Product(operation) => format!("product switch ended: {}", operation.reason),
        };
        let mut error = Self::new(ErrorKind::OperationFailed, message);
        error.terminal = Some(Box::new(terminal));
        error
    }
    pub(crate) fn timeout() -> Self {
        Self::new(
            ErrorKind::Timeout,
            "local wait timed out; remote operation was not cancelled",
        )
    }
    pub(crate) fn request(mut self, request_id: &str) -> Self {
        self.request_id = Some(request_id.to_owned());
        self.submission_unknown = true;
        self
    }
    pub(crate) fn rpc(status: tonic::Status, request_id: Option<&str>) -> Self {
        let code = status.code();
        let mut error = Self::new(ErrorKind::Rpc(code), status.message());
        error.request_id = request_id.map(str::to_owned);
        // Tonic may report OutOfRange after a write when its reply exceeds the receive limit.
        error.submission_unknown = request_id.is_some()
            && matches!(
                code,
                Code::Cancelled
                    | Code::Unknown
                    | Code::DeadlineExceeded
                    | Code::Internal
                    | Code::Unavailable
                    | Code::DataLoss
                    | Code::OutOfRange
            );
        if let Some(value) = status.metadata().get_bin("doso-error-bin") {
            let detail = value
                .to_bytes()
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    rpc::ErrorDetail::decode(bytes).map_err(|error| error.to_string())
                });
            match detail {
                Ok(detail) => error.detail = Some(Box::new(detail)),
                Err(reason) => error
                    .message
                    .push_str(&format!("; invalid DOSO error details: {reason}")),
            }
        }
        error
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_codes() {
        for (kind, code) in [
            (ErrorKind::InvalidInput, Code::InvalidArgument),
            (ErrorKind::Transport, Code::Unavailable),
            (ErrorKind::Protocol, Code::Internal),
            (ErrorKind::Timeout, Code::DeadlineExceeded),
            (ErrorKind::OperationFailed, Code::Unknown),
            (
                ErrorKind::Rpc(Code::PermissionDenied),
                Code::PermissionDenied,
            ),
        ] {
            assert_eq!(Error::new(kind, "test").code(), code);
        }
    }
}
