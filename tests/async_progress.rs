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

#[test]
fn joined_workers_share_global_and_escaped_mutable_storage() {
    run_both(
        r#"
mut int[] global = [1, 2]
fn update_global(int delta) int {
    global[0] = global[0] + delta
    global = global <> [delta]
    return global[0]
}
struct Counter { (fn(int) int) update (fn() int) read }
fn counter(int initial) Counter {
    mut int value = initial
    fn update(int delta) int { value = value + delta;return value }
    fn read() int { return value }
    return Counter{.update = update, .read = read}
}
Counter first = counter(10)
Counter second = counter(100)
fut int initial = async update_global(3)
int joined = await initial
// No ordinary mutable access overlaps a worker: every phase ends with a join.
global[0] = global[0] + 5
fut int next = async update_global(7)
int joined_next = await next
fut int first_job = async first.update(2)
int first_result = await first_job
fut int second_job = async second.update(4)
int second_result = await second_job
_ = first.update(3)
fut int final_job = async first.update(5)
int final_result = await final_job
test "joined shared storage" {
    assert joined == 4
    assert joined_next == 16
    assert global == [16, 2, 3, 7]
    assert first_result == 12
    assert second_result == 104
    assert final_result == 20
    assert first.read() == 20
    assert second.read() == 104
    @println("shared storage checked")
}
"#,
        "",
        "shared storage checked\n",
    );
}

#[test]
fn async_arguments_and_immutable_captures_keep_pre_release_snapshots() {
    run_both(
        r#"
test "async snapshots" {
    mutex bool released = false
    mut int[][] source = [[1, 2], [3]]
    int[][] frozen = source
    fn captured = fn() int[][] {
        mut bool ready = false
        while not ready { lock released { ready = released } }
        return frozen
    }
    fn argument(int[][] values) int[][] {
        mut bool ready = false
        while not ready { lock released { ready = released } }
        mut int[][] local = values
        local[0][0] = 9
        return local
    }
    fut int[][] from_argument = async argument(source)
    fut int[][] from_capture = async captured()
    source[0][0] = 7
    source[1] = [8, 9]
    source = source <> [[10]]
    lock released { released = true }
    mut int[][] argument_result = await from_argument
    mut int[][] capture_result = await from_capture
    int[][] argument_expected = [[9, 2], [3]]
    int[][] frozen_expected = [[1, 2], [3]]
    assert argument_result == argument_expected
    assert capture_result == frozen_expected
    argument_result[1][0] = 20
    capture_result[0] = [30]
    int[][] source_expected = [[7, 2], [8, 9], [10]]
    int[][] mutated_argument = [[9, 2], [20]]
    int[][] mutated_capture = [[30], [3]]
    assert frozen == frozen_expected
    assert source == source_expected
    assert argument_result == mutated_argument
    assert capture_result == mutated_capture
    @println("snapshots checked")
}
"#,
        "",
        "snapshots checked\n",
    );
}

#[test]
fn escaped_mutex_closures_serialize_exactly_once_updates_and_keep_factory_identity() {
    run_both(
        r#"
struct Counter { (fn(uint, int) void) update (fn() int[]) read }
fn counter(int initial) Counter {
    mutex int[] state = [initial, 0, 0, 0]
    fn update(uint slot, int delta) {
        for i in [0, 0, 0, 0, 0, 0, 0, 0] {
            lock state {
                state[0] = state[0] + delta
                state[slot] = state[slot] + 1
            }
        }
    }
    fn read() int[] { lock state { return state } }
    return Counter{.update = update, .read = read}
}
test "escaped mutex storage" {
    Counter first = counter(10)
    Counter second = counter(100)
    int[] before = first.read()
    fut void a = async first.update(1, 1)
    fut void b = async first.update(2, 2)
    fut void c = async first.update(3, 4)
    fut void d = async second.update(1, 10)
    _ = await c
    _ = await d
    _ = await a
    _ = await b
    mut int[] after = first.read()
    assert after == [66, 8, 8, 8]
    assert second.read() == [180, 8, 0, 0]
    assert before == [10, 0, 0, 0]
    after[0] = -1
    assert first.read() == [66, 8, 8, 8]
    @println("mutex identity checked")
}
"#,
        "",
        "mutex identity checked\n",
    );
}

#[test]
fn async_throw_releases_mutex_before_recovery_and_next_worker() {
    run_both(
        r#"
test "async throw unlocks" {
    mutex int value = 0
    fn fail() int! {
        lock value { value = 7;throw "worker failed" }
        return 99
    }
    fn succeed() int {
        lock value { value = value + 1;return value }
    }
    fut int! failed = async fail()
    int recovered = await failed catch err {
        assert @as(str, err) == "worker failed"
        lock value { assert value == 7;value = 10 }
        break 42
    }
    assert recovered == 42
    fut int next = async succeed()
    assert await next == 11
    lock value { assert value == 11 }
    @println("throw unlock checked")
}
"#,
        "",
        "throw unlock checked\n",
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
