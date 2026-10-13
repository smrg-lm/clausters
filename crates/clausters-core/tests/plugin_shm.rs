//! Ownership, capacity, validation and scheduled-output rules of the plugin plane.
use clausters_core::plugin_shm::*;
use std::alloc::{Layout, alloc_zeroed, dealloc};

struct Memory {
    ptr: *mut u8,
    layout: Layout,
}
impl Memory {
    fn new(config: Config) -> Self {
        let layout = Layout::from_size_align(config.bytes().unwrap(), 64).unwrap();
        // SAFETY: valid bounded layout; initialization precedes endpoint access.
        let ptr = unsafe { alloc_zeroed(layout) };
        assert!(!ptr.is_null());
        unsafe { View::initialize(ptr, layout.size(), config) }.unwrap();
        Self { ptr, layout }
    }
    fn view(&self) -> View<'_> {
        // SAFETY: initialized/aligned/live memory; one producer and consumer in each test.
        unsafe { View::attach(self.ptr, self.layout.size(), 11) }.unwrap()
    }
}
impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: borrowed endpoints ended; allocation matches this layout.
        unsafe {
            dealloc(self.ptr, self.layout);
        }
    }
}
fn config() -> Config {
    Config {
        generation: 11,
        sample_rate: 48000.0,
        sample_origin: 0,
        inputs: 1,
        outputs: 1,
        depth: 4,
        reserved: 0,
    }
}
#[test]
fn attach_checks_size_alignment_layout_and_generation() {
    let c = config();
    let memory = Memory::new(c);
    // SAFETY: byte ranges are allocated; rejection precedes invalid payload access.
    unsafe {
        assert!(View::attach(memory.ptr, memory.layout.size() - 1, 11).is_err());
        assert!(View::attach(memory.ptr.add(1), memory.layout.size() - 1, 11).is_err());
        assert!(View::attach(memory.ptr, memory.layout.size(), 12).is_err());
        memory
            .ptr
            .cast::<u32>()
            .add(1)
            .write(PLUGIN_ABI_VERSION + 1);
        assert!(View::attach(memory.ptr, memory.layout.size(), 11).is_err());
    }
    let mut bad = c;
    bad.inputs = CHANNELS + 1;
    assert!(bad.bytes().is_none());
    bad = c;
    bad.sample_rate = f64::NAN;
    assert!(bad.bytes().is_none());
    bad = c;
    bad.depth = 0;
    assert!(bad.bytes().is_none());
}
#[test]
fn silence_late_discard_and_release_keep_one_timeline() {
    let memory = Memory::new(config());
    let mut producer = memory.view().producer();
    let mut consumer = memory.view().consumer();
    let job = Job::full(0);
    let mut output = [0.0; FRAMES];
    let mut result = EventList::default();
    for _ in 0..5 {
        producer.process(&job, &[1.0; FRAMES], &mut output, &mut result);
    }
    assert_eq!(producer.view().counters().0, 1);
    assert_eq!(output, [0.0; FRAMES]);
    let mut late = consumer.next_job().unwrap();
    late.audio().1.fill(99.0);
    late.finish(EventList::default());
    let mut due = consumer.next_job().unwrap();
    due.audio().1.fill(0.5);
    due.finish(EventList::default());
    assert!(producer.process(&job, &[1.0; FRAMES], &mut output, &mut result));
    assert_eq!(output, [0.5; FRAMES]);
    assert_eq!(producer.view().counters(), (1, 1));
}
#[test]
fn jobs_events_and_hang_fail_explicitly_with_bounded_memory() {
    let memory = Memory::new(config());
    let mut producer = memory.view().producer();
    let mut output = [1.0; FRAMES];
    let mut result = EventList::default();
    let mut job = Job::full(0);
    job.span_count = SPANS as u32 + 1;
    assert!(!producer.process(&job, &[0.0; FRAMES], &mut output, &mut result));
    assert_eq!(producer.view().failure(), Failure::Capacity as u32);
    assert_eq!(output, [0.0; FRAMES]);
    let memory = Memory::new(config());
    let mut producer = memory.view().producer();
    for _ in 0..12 {
        producer.process(&Job::full(0), &[0.0; FRAMES], &mut output, &mut result);
    }
    assert_eq!(producer.view().failure(), Failure::Starved as u32);
    let mut events = EventList::default();
    for _ in 0..EVENTS {
        assert!(events.push(Event {
            kind: EventKind::NoteOff as u32,
            ..Event::default()
        }));
    }
    assert!(!events.push(Event {
        kind: EventKind::NoteOff as u32,
        ..Event::default()
    }));
}
#[test]
fn process_guard_never_releases_a_busy_slot_after_failure() {
    let memory = Memory::new(config());
    let mut producer = memory.view().producer();
    let mut consumer = memory.view().consumer();
    let mut output = [0.0; FRAMES];
    let mut result = EventList::default();
    producer.process(&Job::full(0), &[1.0; FRAMES], &mut output, &mut result);
    drop(consumer.next_job().unwrap());
    assert_eq!(producer.view().failure(), Failure::Process as u32);
    assert!(!producer.process(&Job::full(0), &[1.0; FRAMES], &mut output, &mut result));
    assert!(consumer.next_job().is_none());
}

#[test]
fn late_audio_does_not_drop_an_accepted_note_release() {
    let memory = Memory::new(config());
    let mut producer = memory.view().producer();
    let mut consumer = memory.view().consumer();
    let mut output = [0.0; FRAMES];
    let mut result = EventList::default();
    let release = Event {
        offset: 17,
        kind: EventKind::NoteOff as u32,
        note_id: 42,
        key: 60,
        ..Event::default()
    };
    let mut job = Job::full(0);
    assert!(job.events.push(release));
    producer.process(&job, &[1.0; FRAMES], &mut output, &mut result);
    // Keep the consumer idle until the release's output deadline has passed.
    for sequence in 1..=DEFAULT_DEPTH {
        producer.process(
            &Job::full(sequence as i64 * FRAMES as i64),
            &[1.0; FRAMES],
            &mut output,
            &mut result,
        );
    }
    assert_eq!(producer.view().counters(), (1, 0));
    let mut late = consumer.next_job().unwrap();
    assert_eq!(late.job().sequence, 0);
    assert_eq!(late.job().events.count, 1);
    assert_eq!(late.job().events.values[0], release);
    late.audio().1.fill(99.0);
    let mut late_events = EventList::default();
    assert!(late_events.push(release));
    late.finish(late_events);
    let mut due = consumer.next_job().unwrap();
    assert_eq!(due.job().sequence, 1);
    assert_eq!(due.job().events.count, 0);
    due.audio().1.fill(0.5);
    due.finish(EventList::default());
    assert!(producer.process(&Job::full(320), &[1.0; FRAMES], &mut output, &mut result));
    assert_eq!(output, [0.5; FRAMES]);
    assert_eq!(result.count, 0, "late output events must not move in time");
    assert_eq!(producer.view().counters(), (1, 1));
    assert_eq!(producer.view().failure(), Failure::None as u32);
}

#[test]
fn failure_preserves_the_helpers_busy_payload_until_it_finishes() {
    let memory = Memory::new(config());
    let mut producer = memory.view().producer();
    let mut consumer = memory.view().consumer();
    let mut output = [0.0; FRAMES];
    let mut result = EventList::default();
    producer.process(&Job::full(0), &[0.25; FRAMES], &mut output, &mut result);
    let mut busy = consumer.next_job().unwrap();
    for _ in 0..SLOTS * 2 {
        producer.process(&Job::full(64), &[0.75; FRAMES], &mut output, &mut result);
    }
    assert_eq!(producer.view().failure(), Failure::Starved as u32);
    assert_eq!(busy.job().sequence, 0);
    assert_eq!(busy.audio().0, &[0.25; FRAMES]);
    busy.audio().1.fill(1.0);
    busy.finish(EventList::default());
    assert!(!producer.process(&Job::full(128), &[0.75; FRAMES], &mut output, &mut result));
    assert_eq!(output, [0.0; FRAMES]);
    assert!(consumer.next_job().is_none());
}
