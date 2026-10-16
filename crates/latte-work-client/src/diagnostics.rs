//! Bounded startup diagnostics. After Hello, stderr is drained without retaining
//! Agent/application output; dropping the transport also cancels its reader.
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    task::JoinHandle,
};

const LIMIT: usize = 8 * 1024;

pub(super) struct StartupDiagnostics {
    tail: Arc<Mutex<Option<VecDeque<u8>>>>,
    task: JoinHandle<()>,
}

impl StartupDiagnostics {
    pub(super) fn start(mut stderr: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let tail = Arc::new(Mutex::new(Some(VecDeque::with_capacity(LIMIT))));
        let retained = Arc::clone(&tail);
        let task = tokio::spawn(async move {
            let mut bytes = [0; 1024];
            loop {
                let count = match stderr.read(&mut bytes).await {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let mut retained = retained.lock().expect("startup diagnostics lock");
                if let Some(tail) = retained.as_mut() {
                    let excess = (tail.len() + count).saturating_sub(LIMIT);
                    tail.drain(..excess);
                    tail.extend(&bytes[..count]);
                }
            }
        });
        Self { tail, task }
    }

    pub(super) fn discard(&self) {
        *self.tail.lock().expect("startup diagnostics lock") = None;
    }

    pub(super) async fn failure_detail(&mut self) -> String {
        // A descendant may keep stderr open after SSH exits. Never let collecting
        // diagnostics turn a bounded connection attempt into an unbounded wait.
        let _ = tokio::time::timeout(Duration::from_millis(250), &mut self.task).await;
        let tail = self.tail.lock().expect("startup diagnostics lock");
        let Some(tail) = tail.as_ref() else {
            return String::new();
        };
        let bytes: Vec<_> = tail.iter().copied().collect();
        String::from_utf8_lossy(&bytes)
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect::<String>()
            .trim()
            .to_owned()
    }
}

impl Drop for StartupDiagnostics {
    fn drop(&mut self) {
        self.task.abort();
    }
}
