//! Where the CPU's time goes. The embassy executor calls hooks around each
//! task poll (embassy-executor's `trace` feature) and the RTOS scheduler
//! calls hooks as it switches threads (esp-rtos's `rtos-trace`). The hooks
//! only add cycle counts to counters; composition takes them into the
//! trace's activity record every few seconds. A poll's time includes every
//! interrupt taken and every higher-priority thread run during it.
//!
//! The executor's hooks run in its thread and the scheduler's in its
//! interrupt, on the one core the RTOS runs, so each counter has one writer
//! at a time: a hook loads and stores, and taking the scheduler's counters
//! masks interrupts. The scheduler's hooks stay in flash: it raises them at
//! interrupt priority 1, which a flash operation's critical section masks
//! while the cache is off.
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering::Relaxed};

/// The tasks composition spawns, by their names in the trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Task {
    Main,
    Console,
    Power,
    Positioning,
    Io,
    Bluetooth,
    WifiConnection,
    WifiStation,
    WifiAccessPoint,
    WifiVerify,
    TimeSync,
    Dhcp,
    Http,
    Maintenance,
}

impl Task {
    const ALL: [Self; 14] = [
        Self::Main,
        Self::Console,
        Self::Power,
        Self::Positioning,
        Self::Io,
        Self::Bluetooth,
        Self::WifiConnection,
        Self::WifiStation,
        Self::WifiAccessPoint,
        Self::WifiVerify,
        Self::TimeSync,
        Self::Dhcp,
        Self::Http,
        Self::Maintenance,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Console => "console",
            Self::Power => "power",
            Self::Positioning => "gps",
            Self::Io => "io",
            Self::Bluetooth => "ble",
            Self::WifiConnection => "wifi",
            Self::WifiStation => "wifi.station",
            Self::WifiAccessPoint => "wifi.ap",
            Self::WifiVerify => "wifi.verify",
            Self::TimeSync => "time",
            Self::Dhcp => "dhcp",
            Self::Http => "http",
            Self::Maintenance => "maintenance",
        }
    }
}

/// Tasks counted apart; one spawned beyond them counts in none.
const SLOTS: usize = 24;
const UNNAMED: u8 = u8::MAX;
/// Cycle counts fold into units of this many cycles, so a counter wraps
/// after 2^52 cycles rather than 18 s.
const FOLD_BITS: u32 = 20;

/// Cycles since the counter was last taken.
struct Cycles {
    low: AtomicU32,
    high: AtomicU32,
}

impl Cycles {
    const fn new() -> Self {
        Self {
            low: AtomicU32::new(0),
            high: AtomicU32::new(0),
        }
    }

    fn add(&self, cycles: u32) {
        let total = u64::from(self.low.load(Relaxed)) + u64::from(cycles);
        let high = self
            .high
            .load(Relaxed)
            .wrapping_add((total >> FOLD_BITS) as u32);
        self.high.store(high, Relaxed);
        self.low
            .store((total & ((1 << FOLD_BITS) - 1)) as u32, Relaxed);
    }

    fn take(&self) -> u64 {
        let high = self.high.swap(0, Relaxed);
        let low = self.low.swap(0, Relaxed);
        (u64::from(high) << FOLD_BITS) + u64::from(low)
    }
}

struct Slot {
    /// The task's address, its id in the hooks; zero while free.
    id: AtomicU32,
    task: AtomicU8,
    polls: AtomicU32,
    busy: Cycles,
}

impl Slot {
    const fn new() -> Self {
        Self {
            id: AtomicU32::new(0),
            task: AtomicU8::new(UNNAMED),
            polls: AtomicU32::new(0),
            busy: Cycles::new(),
        }
    }
}

/// What the CPU was running, as the scheduler last switched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Thread {
    Scheduler,
    Idle,
    Main,
    Radio,
}

struct Counters {
    slots: [Slot; SLOTS],
    /// The slot of the task being polled, and the cycle count then.
    polled: AtomicU32,
    poll_began: AtomicU32,
    wakes: AtomicU32,
    woke_at: AtomicU32,
    awake: Cycles,
    /// The thread the executor runs in: the first the scheduler made.
    main: AtomicU32,
    thread: AtomicU8,
    switched_at: AtomicU32,
    threads: [Cycles; 4],
    switches: AtomicU32,
    ticks: AtomicU32,
}

/// One task's polls and busy time since the counters were last taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskTime {
    pub task: Option<Task>,
    pub polls: u32,
    pub busy_us: u32,
}

/// The counters as [`Counters::take`] took them, times in µs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Taken {
    pub wakes: u32,
    pub awake_us: u32,
    pub main_us: u32,
    pub radio_us: u32,
    pub idle_us: u32,
    pub switches: u32,
    pub ticks: u32,
    /// Calls to the hooks that count, for their cost.
    pub calls: u32,
    pub tasks: [Option<TaskTime>; SLOTS],
}

impl Counters {
    const fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; SLOTS],
            polled: AtomicU32::new(SLOTS as u32),
            poll_began: AtomicU32::new(0),
            wakes: AtomicU32::new(0),
            woke_at: AtomicU32::new(0),
            awake: Cycles::new(),
            main: AtomicU32::new(0),
            thread: AtomicU8::new(Thread::Main as u8),
            switched_at: AtomicU32::new(0),
            threads: [const { Cycles::new() }; 4],
            switches: AtomicU32::new(0),
            ticks: AtomicU32::new(0),
        }
    }

    /// The slot holding `id`, or a free one now claimed for it.
    fn slot(&self, id: u32) -> Option<usize> {
        let mut free = None;
        for (index, slot) in self.slots.iter().enumerate() {
            match slot.id.load(Relaxed) {
                held if held == id => return Some(index),
                0 if free.is_none() => free = Some(index),
                _ => {}
            }
        }
        let index = free?;
        self.slots[index].id.store(id, Relaxed);
        Some(index)
    }

    fn name(&self, id: u32, task: Task) {
        if let Some(index) = self.slot(id) {
            self.slots[index].task.store(task as u8, Relaxed);
        }
    }

    fn task_new(&self, id: u32) {
        let _ = self.slot(id);
    }

    fn poll_start(&self, now: u32) {
        self.wakes
            .store(self.wakes.load(Relaxed).wrapping_add(1), Relaxed);
        self.woke_at.store(now, Relaxed);
    }

    fn executor_idle(&self, now: u32) {
        self.awake.add(now.wrapping_sub(self.woke_at.load(Relaxed)));
    }

    fn exec_begin(&self, id: u32, now: u32) {
        let index = self
            .slots
            .iter()
            .position(|slot| slot.id.load(Relaxed) == id)
            .unwrap_or(SLOTS);
        if let Some(slot) = self.slots.get(index) {
            slot.polls
                .store(slot.polls.load(Relaxed).wrapping_add(1), Relaxed);
        }
        self.polled.store(index as u32, Relaxed);
        self.poll_began.store(now, Relaxed);
    }

    fn exec_end(&self, now: u32) {
        let index = self.polled.load(Relaxed) as usize;
        if let Some(slot) = self.slots.get(index) {
            slot.busy
                .add(now.wrapping_sub(self.poll_began.load(Relaxed)));
        }
    }

    /// Count the time since the last switch to the thread that ran, and
    /// switch to `next`.
    fn switch(&self, next: Thread, now: u32) {
        let ran = self.thread.load(Relaxed);
        if let Some(time) = self.threads.get(usize::from(ran)) {
            time.add(now.wrapping_sub(self.switched_at.load(Relaxed)));
        }
        self.thread.store(next as u8, Relaxed);
        self.switched_at.store(now, Relaxed);
    }

    fn scheduler_start(&self, now: u32) {
        self.switched_at.store(now, Relaxed);
    }

    fn thread_new(&self, id: u32) {
        if self.main.load(Relaxed) == 0 {
            self.main.store(id, Relaxed);
        }
    }

    fn thread_begin(&self, id: u32, now: u32) {
        let next = if id == self.main.load(Relaxed) {
            Thread::Main
        } else {
            Thread::Radio
        };
        self.switch(next, now);
        self.switches
            .store(self.switches.load(Relaxed).wrapping_add(1), Relaxed);
    }

    fn thread_end(&self, now: u32) {
        self.switch(Thread::Scheduler, now);
    }

    fn idle(&self, now: u32) {
        self.switch(Thread::Idle, now);
    }

    fn tick(&self) {
        self.ticks
            .store(self.ticks.load(Relaxed).wrapping_add(1), Relaxed);
    }

    /// Everything counted since the last take, at `now` in cycles of a
    /// CPU at `mhz`; the thread running now is counted up to `now`. On the
    /// device the scheduler's interrupt must not run meanwhile.
    fn take(&self, now: u32, mhz: u32) -> Taken {
        let us = |cycles: u64| u32::try_from(cycles / u64::from(mhz.max(1))).unwrap_or(u32::MAX);
        self.switch(
            match self.thread.load(Relaxed) {
                t if t == Thread::Idle as u8 => Thread::Idle,
                t if t == Thread::Main as u8 => Thread::Main,
                t if t == Thread::Radio as u8 => Thread::Radio,
                _ => Thread::Scheduler,
            },
            now,
        );
        let mut taken = Taken {
            wakes: self.wakes.swap(0, Relaxed),
            awake_us: us(self.awake.take()),
            main_us: us(self.threads[Thread::Main as usize].take()),
            radio_us: us(self.threads[Thread::Radio as usize].take()),
            idle_us: us(self.threads[Thread::Idle as usize].take()),
            switches: self.switches.swap(0, Relaxed),
            ticks: self.ticks.swap(0, Relaxed),
            ..Taken::default()
        };
        let _ = self.threads[Thread::Scheduler as usize].take();
        let mut polls = 0u32;
        for (slot, out) in self.slots.iter().zip(taken.tasks.iter_mut()) {
            let count = slot.polls.swap(0, Relaxed);
            let busy = slot.busy.take();
            if count == 0 && busy == 0 {
                continue;
            }
            polls = polls.wrapping_add(count);
            *out = Some(TaskTime {
                task: Task::ALL.get(usize::from(slot.task.load(Relaxed))).copied(),
                polls: count,
                busy_us: us(busy),
            });
        }
        taken.calls = taken
            .wakes
            .wrapping_add(polls)
            .wrapping_add(taken.switches)
            .wrapping_mul(2)
            .wrapping_add(taken.ticks);
        taken
    }
}

#[cfg(feature = "hardware")]
pub use device::*;

#[cfg(feature = "hardware")]
mod device {
    use super::{Counters, Taken, Task};
    use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
    use esp_hal::xtensa_lx::timer::get_cycle_count as cycles;

    static COUNTERS: Counters = Counters::new();
    /// What one counting hook costs, in cycles, as [`calibrate`] measured.
    static HOOK_CYCLES: AtomicU32 = AtomicU32::new(0);
    static PASS_BEGAN_US: AtomicU32 = AtomicU32::new(0);
    static PASS_MAX_US: AtomicU32 = AtomicU32::new(0);

    /// Spawn `token` as `task`, under that name in the trace.
    pub fn spawn<S>(
        spawner: embassy_executor::Spawner,
        task: Task,
        token: embassy_executor::SpawnToken<S>,
    ) {
        COUNTERS.name(token.id(), task);
        spawner.spawn(token);
    }

    /// Name the task that awaits this.
    pub async fn name_current(task: Task) {
        let id = core::future::poll_fn(|context| {
            core::task::Poll::Ready(embassy_executor::raw::task_from_waker(context.waker()).id())
        })
        .await;
        COUNTERS.name(id, task);
    }

    /// The counters since the last take, and what counting them cost.
    pub fn take() -> (Taken, u32) {
        let mhz = esp_hal::clock::cpu_clock().as_mhz();
        let taken = critical_section::with(|_| COUNTERS.take(cycles(), mhz));
        let hooks_us =
            u64::from(taken.calls) * u64::from(HOOK_CYCLES.load(Relaxed)) / u64::from(mhz.max(1));
        (taken, u32::try_from(hooks_us).unwrap_or(u32::MAX))
    }

    /// Measure what a counting hook costs at most, on counters of their
    /// own with a task in the last slot a poll searches, once at startup;
    /// logged, and used to estimate each window's cost.
    #[inline(never)]
    pub fn calibrate() {
        const PAIRS: u32 = 64;
        let scratch = Counters::new();
        for id in 1..=super::SLOTS as u32 {
            scratch.task_new(id);
        }
        let id = core::hint::black_box(super::SLOTS as u32);
        let started = cycles();
        for _ in 0..PAIRS {
            pair(&scratch, id);
        }
        let per_call = cycles().wrapping_sub(started) / (2 * PAIRS);
        HOOK_CYCLES.store(per_call, Relaxed);
        log::info!(target: "activity", "hook_cycles={per_call}");
    }

    #[inline(never)]
    fn pair(counters: &Counters, id: u32) {
        counters.exec_begin(id, cycles());
        counters.exec_end(cycles());
    }

    /// A main-loop pass begins; one that awaits restarts after the await.
    pub fn pass_began() {
        PASS_BEGAN_US.store(now_us(), Relaxed);
    }

    pub fn pass_ended() {
        let took = now_us().wrapping_sub(PASS_BEGAN_US.load(Relaxed));
        PASS_MAX_US.fetch_max(took, Relaxed);
    }

    /// The slowest pass since the last take.
    pub fn take_pass_max_us() -> u32 {
        PASS_MAX_US.swap(0, Relaxed)
    }

    fn now_us() -> u32 {
        embassy_time::Instant::now().as_micros() as u32
    }

    #[unsafe(no_mangle)]
    fn _embassy_trace_poll_start(_executor: u32) {
        COUNTERS.poll_start(cycles());
    }

    #[unsafe(no_mangle)]
    fn _embassy_trace_executor_idle(_executor: u32) {
        COUNTERS.executor_idle(cycles());
    }

    #[unsafe(no_mangle)]
    fn _embassy_trace_task_new(_executor: u32, task: u32) {
        COUNTERS.task_new(task);
    }

    #[unsafe(no_mangle)]
    fn _embassy_trace_task_end(_executor: u32, _task: u32) {}

    #[unsafe(no_mangle)]
    fn _embassy_trace_task_exec_begin(_executor: u32, task: u32) {
        COUNTERS.exec_begin(task, cycles());
    }

    #[unsafe(no_mangle)]
    fn _embassy_trace_task_exec_end(_executor: u32, _task: u32) {
        COUNTERS.exec_end(cycles());
    }

    #[unsafe(no_mangle)]
    fn _embassy_trace_task_ready_begin(_executor: u32, _task: u32) {}

    struct Scheduler;

    impl rtos_trace::RtosTrace for Scheduler {
        fn start() {
            COUNTERS.scheduler_start(cycles());
        }
        fn stop() {}
        fn task_new(id: u32) {
            COUNTERS.thread_new(id);
        }
        fn task_send_info(_: u32, _: rtos_trace::TaskInfo) {}
        fn task_new_stackless(_: u32, _: &'static str, _: u32) {}
        fn task_terminate(_: u32) {}
        fn task_exec_begin(id: u32) {
            COUNTERS.thread_begin(id, cycles());
        }
        fn task_exec_end() {
            COUNTERS.thread_end(cycles());
        }
        fn task_ready_begin(_: u32) {}
        fn task_ready_end(_: u32) {}
        fn system_idle() {
            COUNTERS.idle(cycles());
        }
        fn isr_enter() {}
        fn isr_exit() {}
        fn isr_exit_to_scheduler() {}
        fn name_marker(_: u32, _: &'static str) {}
        fn marker(_: u32) {}
        fn marker_begin(id: u32) {
            if id == esp_rtos::TraceEvents::TimerTickHandler as u32 {
                COUNTERS.tick();
            }
        }
        fn marker_end(_: u32) {}
    }

    rtos_trace::global_trace!(Scheduler);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MHZ: u32 = 240;

    #[test]
    fn a_task_counts_its_polls_and_the_cycles_inside_them() {
        let counters = Counters::new();
        counters.name(0x100, Task::Console);
        counters.task_new(0x200);
        counters.poll_start(0);
        counters.exec_begin(0x100, 0);
        counters.exec_end(2_400);
        counters.exec_begin(0x200, 2_400);
        counters.exec_end(2_640);
        counters.executor_idle(3_000);
        counters.poll_start(240_000);
        counters.exec_begin(0x100, 240_000);
        counters.exec_end(242_400);
        counters.executor_idle(242_400);
        let taken = counters.take(480_000, MHZ);
        assert_eq!((taken.wakes, taken.awake_us), (2, 22));
        assert_eq!(
            taken.tasks[0],
            Some(TaskTime {
                task: Some(Task::Console),
                polls: 2,
                busy_us: 20
            })
        );
        assert_eq!(
            taken.tasks[1],
            Some(TaskTime {
                task: None,
                polls: 1,
                busy_us: 1
            })
        );
        let again = counters.take(480_000, MHZ);
        assert_eq!(again.tasks, [None; SLOTS], "a take starts the next window");
    }

    #[test]
    fn each_thread_switch_closes_the_time_of_the_one_before() {
        let counters = Counters::new();
        counters.scheduler_start(0);
        counters.thread_new(0xa);
        counters.thread_new(0xb);
        counters.thread_end(2_400);
        counters.thread_begin(0xb, 2_640);
        counters.thread_end(4_800);
        counters.idle(5_040);
        counters.tick();
        counters.thread_begin(0xa, 240_000);
        let taken = counters.take(264_000, MHZ);
        assert_eq!(
            (taken.main_us, taken.radio_us, taken.idle_us),
            (10 + 100, 9, 979)
        );
        assert_eq!((taken.switches, taken.ticks), (2, 1));
    }

    #[test]
    fn cycle_counts_survive_the_counter_wrapping() {
        let counters = Counters::new();
        counters.task_new(1);
        for round in 0..10u32 {
            let at = round.wrapping_mul(0x9000_0000);
            counters.exec_begin(1, at);
            counters.exec_end(at.wrapping_add(0x8000_0000));
        }
        let taken = counters.take(0, MHZ);
        assert_eq!(
            taken.tasks[0].map(|task| task.busy_us),
            Some((10 * 0x8000_0000u64 / u64::from(MHZ)) as u32)
        );
    }

    #[test]
    fn a_task_beyond_the_slots_is_counted_in_none() {
        let counters = Counters::new();
        for id in 1..=SLOTS as u32 + 1 {
            counters.task_new(id);
        }
        counters.exec_begin(SLOTS as u32 + 1, 0);
        counters.exec_end(240);
        let taken = counters.take(240, MHZ);
        assert!(taken.tasks.iter().all(Option::is_none), "{taken:?}");
    }
}
