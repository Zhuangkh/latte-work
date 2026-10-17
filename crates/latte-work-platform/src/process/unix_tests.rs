use super::*;
use std::{cell::Cell, collections::VecDeque};

fn result(signal: Signal, error: Errno) -> io::Result<()> {
    signal_result(signal, Pid::from_raw(1234), Err(error))
}

#[tokio::test(start_paused = true)]
async fn permission_race_retries_until_signal_success_or_missing_group() {
    for last in [Ok(()), Err(Errno::ESRCH)] {
        let mut attempts = VecDeque::from([Err(Errno::EPERM), Err(Errno::EPERM), last]);
        let reaped = Cell::new(0);
        kill_with_retry(
            Instant::now() + Duration::from_secs(1),
            || {
                signal_result(
                    Signal::SIGKILL,
                    Pid::from_raw(1234),
                    attempts.pop_front().unwrap(),
                )
            },
            || {
                reaped.set(reaped.get() + 1);
                Ok(())
            },
            true,
        )
        .await
        .unwrap();
        assert!(attempts.is_empty());
        assert_eq!(reaped.get(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn permission_race_deadline_preserves_last_error_even_after_root_reap() {
    let attempts = Cell::new(0);
    let reaped = Cell::new(0);
    let started = Instant::now();
    let error = kill_with_retry(
        started + Duration::from_millis(35),
        || {
            attempts.set(attempts.get() + 1);
            signal_result(
                Signal::SIGKILL,
                Pid::from_raw(attempts.get()),
                Err(Errno::EPERM),
            )
        },
        || {
            reaped.set(reaped.get() + 1);
            Ok(())
        },
        true,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("cleanup deadline exceeded"));
    let last = &error
        .get_ref()
        .unwrap()
        .downcast_ref::<CleanupDeadlineError>()
        .unwrap()
        .0;
    let last = last
        .get_ref()
        .unwrap()
        .downcast_ref::<SignalError>()
        .unwrap();
    assert_eq!(last.source, Errno::EPERM);
    assert_eq!(last.pgid.as_raw(), attempts.get());
    assert_eq!(reaped.get(), attempts.get());
    assert!(started.elapsed() >= Duration::from_millis(35));
    // Tokio's timer wheel may round the final deadline up to a millisecond.
    assert!(started.elapsed() <= Duration::from_millis(36));
}

#[tokio::test(start_paused = true)]
async fn expired_budget_does_not_start_a_new_retry_window() {
    let attempts = Cell::new(0);
    let error = kill_with_retry(
        Instant::now() - Duration::from_secs(1),
        || {
            attempts.set(attempts.get() + 1);
            result(Signal::SIGKILL, Errno::EPERM)
        },
        || Ok(()),
        true,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(attempts.get(), 1);
}

#[tokio::test(start_paused = true)]
async fn other_signal_errors_and_non_darwin_permission_errors_do_not_retry() {
    for (signal, errno, retry) in [
        (Signal::SIGTERM, Errno::EPERM, true),
        (Signal::SIGKILL, Errno::EACCES, true),
        (Signal::SIGKILL, Errno::EIO, true),
        (Signal::SIGKILL, Errno::EPERM, false),
    ] {
        let attempts = Cell::new(0);
        let error = kill_with_retry(
            Instant::now() + Duration::from_secs(1),
            || {
                attempts.set(attempts.get() + 1);
                result(signal, errno)
            },
            || panic!("non-retryable signal error polled the root"),
            retry,
        )
        .await
        .unwrap_err();
        assert_eq!(attempts.get(), 1);
        assert!(!error.to_string().contains("deadline"));
    }
    assert!(!retryable_kill(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
}

#[tokio::test(start_paused = true)]
async fn root_poll_failure_stops_retrying() {
    let attempts = Cell::new(0);
    let error = kill_with_retry(
        Instant::now() + Duration::from_secs(1),
        || {
            attempts.set(attempts.get() + 1);
            result(Signal::SIGKILL, Errno::EPERM)
        },
        || Err(io::Error::other("root wait failed")),
        true,
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "root wait failed");
    assert_eq!(attempts.get(), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelling_retry_stops_signal_attempts() {
    let attempts = Cell::new(0);
    {
        let pending = kill_with_retry(
            Instant::now() + Duration::from_secs(1),
            || {
                attempts.set(attempts.get() + 1);
                result(Signal::SIGKILL, Errno::EPERM)
            },
            || Ok(()),
            true,
        );
        tokio::pin!(pending);
        tokio::select! {
            result = &mut pending => panic!("retry completed before cancellation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(25)) => {},
        }
    }
    let before = attempts.get();
    assert!(before > 0);
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(attempts.get(), before);
}
