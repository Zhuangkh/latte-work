use anyhow::{Result, bail};
use latte_work_protocol::Response;
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
const MESSAGE: &str = "此 Windows Host 尚不支持 PTY 终端";
pub const SUPPORTED: bool = false;
#[derive(Default)]
pub struct Terminals {
    _unsupported: (),
}
pub type SharedTerminals = Arc<Mutex<Terminals>>;
impl Terminals {
    pub fn is_open(&self, _: &str) -> bool {
        false
    }
    pub fn is_empty(&self) -> bool {
        true
    }
    pub fn list(&self, _: &str) -> Result<Response> {
        bail!(MESSAGE)
    }
    pub fn create(&mut self, _: String, _: &Path, _: String, _: u16, _: u16) -> Result<Response> {
        bail!(MESSAGE)
    }
    pub fn read(&self, _: &str, _: f64) -> Result<Response> {
        bail!(MESSAGE)
    }
    pub fn resize(&self, _: &str, _: u16, _: u16) -> Result<Response> {
        bail!(MESSAGE)
    }
    pub fn close(&mut self, _: &str) -> Response {
        Response::Error {
            code: "unsupported".into(),
            message: MESSAGE.into(),
        }
    }
    pub fn close_all(&mut self) {}
}
pub async fn write(_: &SharedTerminals, _: &str, _: Vec<u8>) -> Result<Response> {
    bail!(MESSAGE)
}
