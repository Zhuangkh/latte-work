use std::io::{self, Read};

// Count bytes rather than assuming a PTY read fills its buffer. The call bound
// also yields for tiny reads or repeated interruptions without delaying output.
const BYTE_BUDGET: usize = 64 * 1024;
const CALL_BUDGET: usize = 256;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Progress {
    BudgetExhausted,
    WouldBlock,
    Eof,
}

pub(super) fn drain(reader: &mut impl Read, mut append: impl FnMut(&[u8])) -> io::Result<Progress> {
    let mut buffer = [0; 8192];
    let mut bytes = 0;
    for _ in 0..CALL_BUDGET {
        let limit = buffer.len().min(BYTE_BUDGET - bytes);
        match reader.read(&mut buffer[..limit]) {
            Ok(0) => return Ok(Progress::Eof),
            Ok(n) => {
                append(&buffer[..n]);
                bytes += n;
                if bytes == BYTE_BUDGET {
                    return Ok(Progress::BudgetExhausted);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Ok(Progress::WouldBlock);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Progress::BudgetExhausted)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Output {
        remaining: usize,
        grain: usize,
        calls: usize,
    }

    impl Read for Output {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            let n = self.remaining.min(self.grain).min(buffer.len());
            buffer[..n].fill(b'v');
            self.remaining -= n;
            Ok(n)
        }
    }

    #[test]
    fn large_output_drains_across_short_reads_with_bounded_turns() {
        for grain in [1, 127, 1024, 8192] {
            let mut source = Output {
                remaining: 4 * 1024 * 1024,
                grain,
                calls: 0,
            };
            let mut total = 0;
            while source.remaining != 0 {
                let calls_before = source.calls;
                let mut received = 0;
                let progress = drain(&mut source, |data| {
                    assert!(data.iter().all(|byte| *byte == b'v'));
                    received += data.len();
                })
                .unwrap();
                assert!(received > 0 && received <= BYTE_BUDGET);
                assert!(source.calls - calls_before <= CALL_BUDGET);
                assert!(matches!(
                    progress,
                    Progress::BudgetExhausted | Progress::Eof
                ));
                total += received;
            }
            assert_eq!(total, 4 * 1024 * 1024);
            assert_eq!(drain(&mut source, |_| {}).unwrap(), Progress::Eof);
        }
    }

    struct Failure(io::ErrorKind, usize);

    impl Read for Failure {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.1 += 1;
            Err(self.0.into())
        }
    }

    #[test]
    fn blocked_output_stops_immediately_and_interruptions_yield() {
        let mut blocked = Failure(io::ErrorKind::WouldBlock, 0);
        assert_eq!(drain(&mut blocked, |_| {}).unwrap(), Progress::WouldBlock);
        assert_eq!(blocked.1, 1);
        let mut interrupted = Failure(io::ErrorKind::Interrupted, 0);
        assert_eq!(
            drain(&mut interrupted, |_| {}).unwrap(),
            Progress::BudgetExhausted
        );
        assert_eq!(interrupted.1, CALL_BUDGET);
        let mut failed = Failure(io::ErrorKind::BrokenPipe, 0);
        assert_eq!(
            drain(&mut failed, |_| {}).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(failed.1, 1);
    }
}
