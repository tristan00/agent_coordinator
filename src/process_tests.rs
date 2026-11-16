use super::*;
use crate::test_support::Temp;

#[test]
fn a_job_reports_the_cpu_its_processes_used_and_records_it_for_the_attempt() {
    let temp = Temp::new();
    let directory = temp.dir("attempt");
    assert_eq!(cpu_seconds(&directory), None);
    let job = Job::new().unwrap();
    let mut child = command("powershell").args(["-NoProfile", "-Command", "$s=[Diagnostics.Stopwatch]::StartNew(); while ($s.ElapsedMilliseconds -lt 700) { }"]).spawn().unwrap();
    job.contain(&child).unwrap();
    child.wait().unwrap();
    record_cpu(&job, &directory);
    let used = cpu_seconds(&directory).unwrap();
    assert!(used > 0.2, "{used}");
    assert!((job.cpu_seconds().unwrap() - used).abs() < 0.5);
}
