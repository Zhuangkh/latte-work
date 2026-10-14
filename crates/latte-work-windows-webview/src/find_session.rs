use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// Cancellation shared by the command owner and main-thread Find callbacks.
#[derive(Clone, Default)]
pub struct FindSession(Arc<AtomicU64>);

impl FindSession {
    pub fn begin(&self) -> FindRequest {
        let generation = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        FindRequest {
            session: self.clone(),
            generation,
        }
    }

    pub fn cancel(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct FindRequest {
    session: FindSession,
    generation: u64,
}

impl FindRequest {
    pub fn check(&self) -> io::Result<()> {
        if self.session.0.load(Ordering::SeqCst) == self.generation {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "find request cancelled",
            ))
        }
    }
}

#[cfg(any(windows, test))]
pub(crate) trait FindOptions {
    fn term(&self, query: &str) -> io::Result<()>;
    fn highlight_all(&self, value: bool) -> io::Result<()>;
    fn match_word(&self, value: bool) -> io::Result<()>;
    fn case_sensitive(&self, value: bool) -> io::Result<()>;
    fn suppress_dialog(&self, value: bool) -> io::Result<()>;
}

#[cfg(any(windows, test))]
pub(crate) fn configure(options: &impl FindOptions, query: &str) -> io::Result<()> {
    options.term(query)?;
    options.highlight_all(true)?;
    options.match_word(false)?;
    options.case_sensitive(false)?;
    options.suppress_dialog(true)
}

#[cfg(any(windows, test))]
pub(crate) trait FindControl {
    fn stop(&self) -> io::Result<()>;
    fn next(&self) -> io::Result<()>;
    fn previous(&self) -> io::Result<()>;
}

#[cfg(any(windows, test))]
pub(crate) fn stop_if_current(control: &impl FindControl, request: &FindRequest) -> io::Result<()> {
    if request.check().is_err() {
        return Ok(());
    }
    control.stop()
}

#[cfg(any(windows, test))]
pub(crate) fn prepare(
    control: &impl FindControl,
    request: &FindRequest,
    restart: bool,
    backwards: bool,
) -> io::Result<()> {
    request.check()?;
    if restart {
        control.stop()
    } else if backwards {
        control.previous()
    } else {
        control.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
        fail: Option<&'static str>,
    }

    impl Fake {
        fn record(&self, call: String) -> io::Result<()> {
            self.calls.borrow_mut().push(call.clone());
            if self.fail == Some(call.as_str()) {
                Err(io::Error::other("native failure"))
            } else {
                Ok(())
            }
        }
    }

    impl FindOptions for Fake {
        fn term(&self, query: &str) -> io::Result<()> {
            self.record(format!("term:{query}"))
        }
        fn highlight_all(&self, value: bool) -> io::Result<()> {
            self.record(format!("highlight:{value}"))
        }
        fn match_word(&self, value: bool) -> io::Result<()> {
            self.record(format!("word:{value}"))
        }
        fn case_sensitive(&self, value: bool) -> io::Result<()> {
            self.record(format!("case:{value}"))
        }
        fn suppress_dialog(&self, value: bool) -> io::Result<()> {
            self.record(format!("suppress:{value}"))
        }
    }

    impl FindControl for Fake {
        fn stop(&self) -> io::Result<()> {
            self.record("stop".into())
        }
        fn next(&self) -> io::Result<()> {
            self.record("next".into())
        }
        fn previous(&self) -> io::Result<()> {
            self.record("previous".into())
        }
    }

    #[test]
    fn custom_find_suppresses_native_dialog_and_preserves_match_options() {
        let options = Fake::default();
        configure(&options, "needle").unwrap();
        assert_eq!(
            *options.calls.borrow(),
            [
                "term:needle",
                "highlight:true",
                "word:false",
                "case:false",
                "suppress:true"
            ]
        );
        let failing = Fake {
            fail: Some("suppress:true"),
            ..Fake::default()
        };
        assert!(configure(&failing, "needle").is_err());
    }

    #[test]
    fn restart_stops_first_and_continuation_keeps_direction() {
        let session = FindSession::default();
        let control = Fake::default();
        prepare(&control, &session.begin(), true, false).unwrap();
        prepare(&control, &session.begin(), false, true).unwrap();
        prepare(&control, &session.begin(), false, false).unwrap();
        assert_eq!(*control.calls.borrow(), ["stop", "previous", "next"]);
    }

    #[test]
    fn stop_failure_prevents_restart() {
        let control = Fake {
            fail: Some("stop"),
            ..Fake::default()
        };
        assert!(prepare(&control, &FindSession::default().begin(), true, false).is_err());
        assert_eq!(*control.calls.borrow(), ["stop"]);
    }

    #[test]
    fn stale_reset_never_stops_a_replacement_session() {
        let session = FindSession::default();
        let reset = session.begin();
        let replacement = session.begin();
        let control = Fake::default();
        stop_if_current(&control, &reset).unwrap();
        assert!(control.calls.borrow().is_empty());
        stop_if_current(&control, &replacement).unwrap();
        assert_eq!(*control.calls.borrow(), ["stop"]);
    }

    #[test]
    fn cancelled_work_never_touches_native_find_and_old_completion_stays_cancelled() {
        let session = FindSession::default();
        let request = session.begin();
        session.cancel();
        let control = Fake::default();
        assert_eq!(
            prepare(&control, &request, true, false).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(control.calls.borrow().is_empty());
        let replacement = session.begin();
        assert!(request.check().is_err());
        assert!(replacement.check().is_ok());
    }
}
