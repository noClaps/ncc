use std::{
    fs,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn worker_progress_and_caller_continuation_precede_await() {
    run_both(
        r#"
test "pre-await handshake" {
    mutex bool started = false
    mutex bool released = false
    fn worker() int {
        lock started { started = true }
        mut bool ready = false
        while not ready { lock released { ready = released } }
        return 42
    }
    fut int work = async worker()
    mut bool ready = false
    while not ready { lock started { ready = started } }
    // The worker has progressed, but cannot finish until the caller continues.
    lock released { assert not released;released = true }
    assert await work == 42
    @println("progress checked")
}
"#,
        "",
        "progress checked\n",
    );
}

#[test]
fn held_mutex_forces_contention_before_worker_enters() {
    run_both(
        r#"
extern "probe.c" as probe {
    fn arm() int = "probe_arm"
    fn wait() int = "probe_wait"
    fn begin() int = "probe_begin"
}
test "forced contention" {
    mutex int value = 0
    fn worker() int {
        _ = probe.arm()
        lock value {
            value = value + 1
            return value
        }
    }
    fut int work = async worker()
    lock value {
        _ = probe.begin()
        _ = probe.wait()
        // The probe has observed an actual failed acquisition of this lock.
        assert value == 0
        value = 7
    }
    assert await work == 8
    lock value { assert value == 8 }
    @println("contention checked")
}
"#,
        CONTENTION_PROBE,
        "contention checked\n",
    );
}

#[test]
fn bare_break_releases_mutex_for_caller_and_worker_reacquisition() {
    run_both(
        r#"
test "bare break unlocks" {
    mutex int value = 0
    lock value {
        value = 1
        break
        value = 99
    }
    lock value { assert value == 1;value = 2 }
    fn worker() int {
        lock value {
            value = value + 1
            break
            value = 99
        }
        lock value { value = value + 1;return value }
    }
    fut int work = async worker()
    assert await work == 4
    lock value { assert value == 4 }
    @println("break checked")
}
"#,
        "",
        "break checked\n",
    );
}

// Intercept only the armed worker's next lock. A failed try-lock proves actual
// contention while the caller still owns the NC mutex; the real blocking lock
// then preserves ordinary runtime behavior. Probe synchronization is separate.
const CONTENTION_PROBE: &str = r"
#include <pthread.h>
#include <stdint.h>
#include <errno.h>
#include <stdlib.h>
static pthread_mutex_t probe_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t probe_condition = PTHREAD_COND_INITIALIZER;
static int probe_contended;
static int probe_started;
static _Thread_local int probe_armed;
int64_t probe_begin(void) {
    pthread_mutex_lock(&probe_lock);
    probe_started = 1;
    pthread_cond_broadcast(&probe_condition);
    pthread_mutex_unlock(&probe_lock);
    return 0;
}
int64_t probe_arm(void) {
    pthread_mutex_lock(&probe_lock);
    while (!probe_started) pthread_cond_wait(&probe_condition, &probe_lock);
    pthread_mutex_unlock(&probe_lock);
    probe_armed = 1;
    return 0;
}
int64_t probe_wait(void) {
    pthread_mutex_lock(&probe_lock);
    while (!probe_contended) pthread_cond_wait(&probe_condition, &probe_lock);
    pthread_mutex_unlock(&probe_lock);
    return 0;
}
static int probe_mutex_lock(pthread_mutex_t *mutex) {
    if (probe_armed) {
        probe_armed = 0;
        int result = pthread_mutex_trylock(mutex);
        if (result != EBUSY) abort();
        pthread_mutex_lock(&probe_lock);
        probe_contended = 1;
        pthread_cond_signal(&probe_condition);
        pthread_mutex_unlock(&probe_lock);
    }
    return pthread_mutex_lock(mutex);
}
#define pthread_mutex_lock probe_mutex_lock
";

fn run_both(source: &str, prefix: &str, expected: &str) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("main.nc");
    // The probe's definitions precede the generated unit, so the extern include
    // is empty. No production runtime or compiler instrumentation is required.
    fs::write(directory.path().join("probe.c"), "").unwrap();
    for release in [false, true] {
        let generated = ncc::compile_test_source_with_options(source, &input, release).unwrap();
        let c = directory.path().join("main.c");
        let binary = directory.path().join("program");
        fs::write(&c, format!("{prefix}\n{generated}")).unwrap();
        let output = Command::new("cc")
            .args(["-std=c11", "-pthread", if release { "-O2" } else { "-O0" }])
            .arg(&c)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for repetition in 0..4 {
            let output = bounded_output(&mut Command::new(&binary));
            assert!(
                output.status.success(),
                "release={release}, repetition={repetition}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(output.stdout, expected.as_bytes());
        }
    }
}

fn bounded_output(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        // This is only a deadlock watchdog, never a scheduling assertion.
        if start.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "async fixture deadlocked: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}
