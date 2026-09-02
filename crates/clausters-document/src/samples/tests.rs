use super::*;
use crate::history::History;

fn write(channel: u32, start: u64, values: &[f32]) -> Opaque {
    payload(&SamplesIntent::Write {
        channel,
        start,
        values: values.to_vec(),
    })
}

fn values(payload: &Opaque) -> Vec<f32> {
    let SamplesIntent::Write { values, .. } =
        serde_json::from_value::<SamplesIntent>(payload.0.clone()).expect("a samples payload");
    values
}

#[test]
fn a_span_edited_through_a_history_inverts_to_the_values_it_held() {
    let mut history = History::new();
    let take = history.register(SAMPLES);
    let mut data = vec![0.0, 0.1, 0.2, 0.3, 0.4];

    let applied = {
        let mut samples = Samples::interleaved(&mut data, 1);
        history.apply(take, &mut samples, &write(0, 1, &[-1.0, -2.0]), "draw")
    };
    assert!(applied.applied);
    assert_eq!(data, vec![0.0, -1.0, -2.0, 0.3, 0.4]);

    let undone = history.undo().expect("something to undo");
    let mut samples = Samples::interleaved(&mut data, 1);
    for (structure, payload) in undone.legs {
        assert_eq!(structure, take);
        samples.apply(&payload);
    }
    assert_eq!(data, vec![0.0, 0.1, 0.2, 0.3, 0.4], "where it started");
}

#[test]
fn a_channel_is_a_strided_span_and_the_other_channel_keeps_its_shape() {
    // Interleaved stereo: L0 R0 L1 R1 L2 R2.
    let mut data = vec![0.0, 1.0, 0.0, 1.0, 0.0, 1.0];
    let mut samples = Samples::interleaved(&mut data, 2);
    assert_eq!(samples.frames(), 3);

    let applied = samples.apply(&write(0, 1, &[0.5, 0.6]));
    assert!(applied.applied);
    assert_eq!(
        data,
        vec![0.0, 1.0, 0.5, 1.0, 0.6, 1.0],
        "the left channel was written and the right one was not"
    );
}

#[test]
fn a_span_outside_the_samples_is_refused_and_writes_nothing() {
    let mut data = vec![0.0, 0.1];
    let mut samples = Samples::interleaved(&mut data, 1);

    let applied = samples.apply(&write(0, 1, &[9.0, 9.0]));
    assert!(!applied.applied);
    assert_eq!(
        applied.reason.as_deref(),
        Some("that span is not inside these samples")
    );
    assert_eq!(data, vec![0.0, 0.1], "nothing was written");

    let mut samples = Samples::interleaved(&mut data, 1);
    assert!(
        !samples.apply(&write(3, 0, &[9.0])).applied,
        "a channel these samples do not have"
    );
}

#[test]
fn a_resend_is_not_an_edit_and_leaves_no_entry() {
    let mut history = History::new();
    let take = history.register(SAMPLES);
    let mut data = vec![0.0, 0.5, 1.0];

    let applied = {
        let mut samples = Samples::interleaved(&mut data, 1);
        history.apply(take, &mut samples, &write(0, 1, &[0.5]), "draw")
    };
    assert!(!applied.applied, "the span already held that value");
    assert_eq!(history.len(), 0, "and nothing was recorded");
}

#[test]
fn the_inverse_is_read_before_the_edit_lands() {
    let mut data = vec![1.0, 2.0, 3.0];
    let samples = Samples::interleaved(&mut data, 1);
    let inverse = samples
        .current(&write(0, 0, &[9.0, 9.0]))
        .expect("a span it can describe");
    assert_eq!(values(&inverse), vec![1.0, 2.0]);
}

#[test]
fn one_span_written_twice_is_one_undo_and_two_spans_are_two() {
    let mut data = vec![0.0, 0.0, 0.0];
    let samples = Samples::interleaved(&mut data, 1);

    let one = samples.coalesce_key(&write(0, 1, &[1.0])).expect("a key");
    let again = samples.coalesce_key(&write(0, 1, &[2.0])).expect("a key");
    let elsewhere = samples.coalesce_key(&write(0, 2, &[1.0])).expect("a key");
    assert_eq!(one, again, "the same span written again");
    assert_ne!(one, elsewhere, "a different span");
    assert!(one.starts_with(SAMPLES));
}

#[test]
fn an_empty_write_says_nothing_in_both_directions() {
    let mut data = vec![1.0, 2.0];
    {
        let mut samples = Samples::interleaved(&mut data, 1);
        assert!(!samples.apply(&write(0, 0, &[])).applied);
        let inverse = samples.current(&write(0, 0, &[])).expect("the empty write");
        assert!(values(&inverse).is_empty());
    }
    assert_eq!(data, vec![1.0, 2.0]);
}
