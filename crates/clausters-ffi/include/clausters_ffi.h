#ifndef CLAUSTERS_FFI_H
#define CLAUSTERS_FFI_H

/* Generated from crates/clausters-ffi by crates/clausters-checks/tests/c_header.rs; do not edit. */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define CORE_ABI_VERSION 97

#define SHM_INVALID -1

#define SHM_ABSENT -2

#define SHM_TOO_SMALL -3

#define FileHead_BYTES 40

#define Parameter_STEPPED 1

#define Parameter_HIDDEN (1 << 2)

#define Parameter_READ_ONLY (1 << 3)

#define Parameter_AUTOMATABLE (1 << 5)

#define Parameter_LIST (1 << 16)

#define Head_BYTES ((4 + 4) + (6 * 8))

typedef struct FadeShape FadeShape;

typedef struct FfiDocument FfiDocument;

typedef struct FfiEditing FfiEditing;

typedef struct FfiEventSequence FfiEventSequence;

typedef struct FfiHistory FfiHistory;

typedef struct FfiIdSpaces FfiIdSpaces;

typedef struct FfiMultitrack FfiMultitrack;

typedef struct FfiNotesPlayback FfiNotesPlayback;

typedef struct FfiRegistry FfiRegistry;

typedef struct FfiStepRunner FfiStepRunner;

typedef struct FfiWidgetIds FfiWidgetIds;

typedef struct IdShare IdShare;

typedef struct Ratio Ratio;

typedef struct SampleClockModel SampleClockModel;

typedef struct Scheduler Scheduler;

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
typedef struct ScoreHandle ScoreHandle;
#endif

typedef struct ServerShape ServerShape;

typedef struct SpanStats SpanStats;

typedef struct TempoMap TempoMap;

typedef struct Time Time;

typedef struct WsClient WsClient;

typedef struct Shape {
  uint64_t control_buses;
  uint64_t audio_buses;
  uint64_t taps;
  uint64_t tap_frames;
  uint64_t buffer_rows;
  uint64_t controls_offset;
  uint64_t buses_offset;
  uint64_t taps_offset;
  uint64_t buffers_offset;
  uint64_t sample_rate_offset;
  uint64_t clock_offset;
  uint64_t transport_clock_offset;
  uint64_t transport_position_offset;
  uint64_t buffer_row_size;
  uint64_t tap_slot_size;
  uint64_t c2s_offset;
  uint64_t s2c_offset;
  uint64_t ring_capacity;
  uint64_t ring_prefix;
  uint64_t frame_header;
  uint64_t transports;
  uint64_t transport_stride;
} Shape;



















uint32_t clausters_core_abi_version(void);

uint32_t clausters_session_format(void);

size_t clausters_session_migrate(const uint8_t *session,
                                 size_t session_len,
                                 uint8_t *out,
                                 size_t out_cap);

size_t clausters_apps_samples_measures(const uint8_t *request,
                                       size_t request_len,
                                       uint8_t *out,
                                       size_t out_cap);

struct FfiEditing *clausters_apps_editing_new(void);

void clausters_apps_editing_free(struct FfiEditing *e);

size_t clausters_apps_editing_call(struct FfiEditing *e,
                                   const uint8_t *request,
                                   size_t request_len,
                                   uint8_t *out,
                                   size_t out_cap);

size_t clausters_apps_editing_open_notes(struct FfiEditing *e,
                                         struct FfiEventSequence *sequence,
                                         const uint8_t *request,
                                         size_t request_len,
                                         uint8_t *out,
                                         size_t out_cap);

#if defined(CLAUSTERS_FFI_VEROVIO)
size_t clausters_apps_editing_open_score(struct FfiEditing *e,
                                         struct ScoreHandle *score,
                                         const uint8_t *request,
                                         size_t request_len,
                                         uint8_t *out,
                                         size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_VEROVIO)
size_t clausters_apps_editing_open_score_over(struct FfiEditing *e,
                                              struct ScoreHandle *score,
                                              struct FfiEventSequence *sequence,
                                              const uint8_t *request,
                                              size_t request_len,
                                              uint8_t *out,
                                              size_t out_cap);
#endif

int32_t clausters_core_unary(uint32_t op, const float *input, size_t in_len, float *out, size_t n);

int32_t clausters_core_binary(uint32_t op,
                              const float *a,
                              size_t a_len,
                              const float *b,
                              size_t b_len,
                              float *out,
                              size_t n);

int32_t clausters_core_list(uint32_t op,
                            const double *list,
                            size_t n,
                            double a,
                            double b,
                            double *out,
                            size_t out_n);

ptrdiff_t clausters_core_list_len(uint32_t op, size_t n, double a);

double clausters_core_blendat(const double *list, size_t n, double index, uint32_t method);

int32_t clausters_core_bound(uint32_t op,
                             const float *x,
                             size_t x_len,
                             const float *lo,
                             size_t lo_len,
                             const float *hi,
                             size_t hi_len,
                             float *out,
                             size_t n);

double clausters_core_calc(uint32_t op, const double *args, size_t n);

double clausters_core_scale_calc(uint32_t op, double x, const double *list, size_t n, double steps);

int32_t clausters_core_map(uint32_t op,
                           uint32_t clip,
                           const float *input,
                           size_t in_len,
                           float in_lo,
                           float in_hi,
                           float out_lo,
                           float out_hi,
                           float curve,
                           float *out,
                           size_t n);

void clausters_core_whitenoise(uint64_t seed, float *out, size_t n);

void clausters_core_window(int32_t wintype, float *out, size_t n);

size_t clausters_core_peaks_cache_size(size_t n, size_t base_bucket);

size_t clausters_core_peaks_build(const float *samples,
                                  size_t n,
                                  size_t base_bucket,
                                  uint8_t *out,
                                  size_t out_cap);

size_t clausters_core_peaks_multi_cache_size(size_t frames, size_t channels, size_t base_bucket);

size_t clausters_core_peaks_multi_build(const float *samples,
                                        size_t n,
                                        size_t channels,
                                        size_t base_bucket,
                                        uint8_t *out,
                                        size_t out_cap);

size_t clausters_core_stft_cache_size(size_t n, size_t window_size, size_t hop);

size_t clausters_core_stft_build(const float *samples,
                                 size_t n,
                                 size_t window_size,
                                 size_t hop,
                                 float sample_rate,
                                 uint8_t *out,
                                 size_t out_cap);

size_t clausters_core_peaks_multi_update(uint8_t *cache,
                                         size_t cache_len,
                                         const float *samples,
                                         size_t n,
                                         size_t start,
                                         size_t frames);

size_t clausters_core_peaks_multi_empty(size_t frames,
                                        size_t channels,
                                        size_t base_bucket,
                                        uint8_t *out,
                                        size_t out_cap);

size_t clausters_core_peaks_multi_write_buckets(uint8_t *cache,
                                                size_t cache_len,
                                                size_t start_frame,
                                                size_t bucket,
                                                const float *stats,
                                                size_t n);

int32_t clausters_core_correlation(const float *left, const float *right, size_t n, float *out);

int32_t clausters_core_lissajous(const float *left, const float *right, size_t n, float *out);

size_t clausters_core_bundle_requirements(const uint8_t *request,
                                          size_t request_len,
                                          uint8_t *out,
                                          size_t out_cap);

size_t clausters_core_bundle_resolve(const uint8_t *request,
                                     size_t request_len,
                                     uint8_t *out,
                                     size_t out_cap);

size_t clausters_core_bundle_validate(const uint8_t *request,
                                      size_t request_len,
                                      uint8_t *out,
                                      size_t out_cap);

struct SampleClockModel *clausters_clocksync_new(double nominal_rate, size_t window);

void clausters_clocksync_free(struct SampleClockModel *h);

void clausters_clocksync_add_anchor(struct SampleClockModel *h,
                                    double t_local,
                                    int64_t sample,
                                    double rate);

int64_t clausters_clocksync_sample_at(struct SampleClockModel *h, double t_local);

double clausters_clocksync_local_time_of(struct SampleClockModel *h, int64_t sample);

double clausters_clocksync_drift_ppm(struct SampleClockModel *h);

double clausters_clocksync_span(struct SampleClockModel *h);

double clausters_clocksync_rate(struct SampleClockModel *h);

double clausters_clocksync_slope(struct SampleClockModel *h);

double clausters_clocksync_intercept(struct SampleClockModel *h);

struct FfiDocument *clausters_document_open(const uint8_t *json, size_t len);

void clausters_document_free(struct FfiDocument *h);

size_t clausters_document_inverse(struct FfiDocument *h,
                                  const uint8_t *intent,
                                  size_t intent_len,
                                  uint8_t *out,
                                  size_t out_cap);

size_t clausters_document_coalesce_key(const uint8_t *intent,
                                       size_t intent_len,
                                       uint8_t *out,
                                       size_t out_cap);

uint64_t clausters_document_version(struct FfiDocument *h);

size_t clausters_document_snapshot(struct FfiDocument *h, uint8_t *out, size_t out_cap);

size_t clausters_document_apply(struct FfiDocument *h,
                                const uint8_t *intent,
                                size_t intent_len,
                                const uint8_t *against,
                                size_t against_len,
                                double quant,
                                uint8_t *out,
                                size_t out_cap);

size_t clausters_document_resolve(struct FfiDocument *h,
                                  const uint8_t *selection,
                                  size_t selection_len,
                                  const struct TempoMap *tempo,
                                  double frames_per_second,
                                  int32_t in_beats,
                                  uint8_t *out,
                                  size_t out_cap);

size_t clausters_view_not_an_edit(uint8_t *out, size_t out_cap);

size_t clausters_view_props(const uint8_t *kind,
                            size_t kind_len,
                            const uint8_t *facts,
                            size_t facts_len,
                            uint8_t *out,
                            size_t out_cap);

size_t clausters_mixer_defs(const uint8_t *widths,
                            size_t widths_len,
                            size_t master,
                            uint8_t *out,
                            size_t out_cap);

struct FfiEventSequence *clausters_document_sequence_new(const uint8_t *json, size_t len);

void clausters_document_sequence_free(struct FfiEventSequence *h);

size_t clausters_document_sequence_call(struct FfiEventSequence *h,
                                        const uint8_t *request,
                                        size_t request_len,
                                        uint8_t *out,
                                        size_t out_cap);

struct FfiMultitrack *clausters_document_multitrack_new(const uint8_t *json, size_t len);

void clausters_document_multitrack_free(struct FfiMultitrack *h);

size_t clausters_document_multitrack_call(struct FfiMultitrack *h,
                                          const uint8_t *request,
                                          size_t request_len,
                                          uint8_t *out,
                                          size_t out_cap);

size_t clausters_editing_points_props(const double *points,
                                      size_t n,
                                      int32_t hold,
                                      double kept_lo,
                                      double kept_hi,
                                      double held,
                                      uint8_t *out,
                                      size_t out_cap);

size_t clausters_editing_multitrack_props(const uint8_t *multitrack,
                                          size_t multitrack_len,
                                          double rate,
                                          const uint8_t *sources,
                                          size_t sources_len,
                                          uint8_t *out,
                                          size_t out_cap);

size_t clausters_editing_intake(const uint8_t *domain,
                                size_t domain_len,
                                const uint8_t *tag,
                                size_t tag_len,
                                const uint8_t *request,
                                size_t request_len,
                                uint8_t *out,
                                size_t out_cap);

size_t clausters_editing_event_curves(const uint8_t *request,
                                      size_t request_len,
                                      uint8_t *out,
                                      size_t out_cap);

double clausters_editing_default_tempo(void);

struct FfiStepRunner *clausters_editing_runner_new(void);

void clausters_editing_runner_free(struct FfiStepRunner *r);

size_t clausters_editing_runner_call(struct FfiStepRunner *r,
                                     const uint8_t *request,
                                     size_t request_len,
                                     uint8_t *out,
                                     size_t out_cap);

struct FfiNotesPlayback *clausters_editing_notes_playback_new(int32_t transport);

void clausters_editing_notes_playback_free(struct FfiNotesPlayback *p);

size_t clausters_editing_notes_playback_call(struct FfiNotesPlayback *p,
                                             struct FfiEventSequence *sequence,
                                             const uint8_t *request,
                                             size_t request_len,
                                             struct FfiIdSpaces *ids,
                                             uint8_t *out,
                                             size_t out_cap);

size_t clausters_editing_conversation_read(const uint8_t *state,
                                           size_t state_len,
                                           const uint8_t *message,
                                           size_t message_len,
                                           uint8_t *out,
                                           size_t out_cap);

size_t clausters_editing_conversation_answer(const uint8_t *request,
                                             size_t request_len,
                                             uint8_t *out,
                                             size_t out_cap);

size_t clausters_editing_multitrack_names(const uint8_t *multitrack,
                                          size_t multitrack_len,
                                          uint8_t *out,
                                          size_t out_cap);

float clausters_core_shape_value(int32_t shape, float c, float a, float b, float t);

int32_t clausters_core_curve_axis(const double *values, size_t n, int32_t hold, double *out);

double clausters_core_degree_to_midinote(double degree,
                                         double alter,
                                         double octave,
                                         double root,
                                         const float *scale_ptr,
                                         size_t n);

int32_t clausters_core_midinote_to_degree(double midinote,
                                          double octave,
                                          double root,
                                          const float *scale_ptr,
                                          size_t n,
                                          int32_t spelling,
                                          double *out);

int32_t clausters_core_split_degree(double degree, double *out);

int32_t clausters_core_pitch_resolve(const double *keys,
                                     const float *scale_ptr,
                                     size_t n,
                                     double *out);

int32_t clausters_core_pitch_set(double *keys,
                                 uint32_t key,
                                 double value,
                                 const float *scale_ptr,
                                 size_t n,
                                 int32_t spelling);

int32_t clausters_core_level_resolve(const double *keys, double *out);

int32_t clausters_core_level_set(double *keys, uint32_t key, double value);

double clausters_core_amp_of_velocity(double velocity);

double clausters_core_velocity_of_amp(double amp);

double clausters_core_event_delta(double dur, double stretch, double delta);

double clausters_core_event_sustain(double dur, double legato, double stretch, double sustain);

size_t clausters_core_event_synth(const uint8_t *request, size_t len, uint8_t *out, size_t out_cap);

size_t clausters_core_event_node_note(const uint8_t *request,
                                      size_t len,
                                      uint8_t *out,
                                      size_t out_cap);

size_t clausters_core_event_midi(const uint8_t *request, size_t len, uint8_t *out, size_t out_cap);

size_t clausters_core_event_of_midi(const uint8_t *request,
                                    size_t len,
                                    uint8_t *out,
                                    size_t out_cap);

size_t clausters_core_event_notation_keys(uint8_t *out, size_t out_cap);

double clausters_core_event_written_midinote(const uint8_t *pitches, size_t len);

struct FfiHistory *clausters_history_new(size_t budget, size_t spill_above);

void clausters_history_free(struct FfiHistory *h);

uint64_t clausters_history_register(struct FfiHistory *h, const uint8_t *domain, size_t domain_len);

size_t clausters_history_apply(struct FfiHistory *h,
                               uint64_t structure,
                               struct FfiDocument *doc,
                               const uint8_t *intent,
                               size_t intent_len,
                               const uint8_t *against,
                               size_t against_len,
                               double quant,
                               const uint8_t *label,
                               size_t label_len,
                               uint8_t *out,
                               size_t out_cap);

int32_t clausters_history_record(struct FfiHistory *h, const uint8_t *request, size_t request_len);

size_t clausters_history_walk(struct FfiHistory *h,
                              const uint8_t *direction,
                              size_t direction_len,
                              uint8_t *out,
                              size_t out_cap);

int32_t clausters_history_forget(struct FfiHistory *h, uint64_t structure);

size_t clausters_history_released(struct FfiHistory *h, uint8_t *out, size_t out_cap);

void clausters_history_mark_saved(struct FfiHistory *h);

int32_t clausters_history_dirty(struct FfiHistory *h);

int32_t clausters_history_saved_reachable(struct FfiHistory *h);

int32_t clausters_history_can_undo(struct FfiHistory *h);

int32_t clausters_history_can_redo(struct FfiHistory *h);

size_t clausters_history_undo_label(struct FfiHistory *h, uint8_t *out, size_t out_cap);

size_t clausters_history_redo_label(struct FfiHistory *h, uint8_t *out, size_t out_cap);

size_t clausters_history_len(struct FfiHistory *h);

void clausters_history_clear(struct FfiHistory *h);

size_t clausters_domain_coalesce_key(const uint8_t *domain,
                                     size_t domain_len,
                                     const uint8_t *payload,
                                     size_t payload_len,
                                     uint8_t *out,
                                     size_t out_cap);

size_t clausters_domain_edit(const uint8_t *domain,
                             size_t domain_len,
                             const uint8_t *state,
                             size_t state_len,
                             const uint8_t *payload,
                             size_t payload_len,
                             uint8_t *out,
                             size_t out_cap);

size_t clausters_domain_edit_all(const uint8_t *domain,
                                 size_t domain_len,
                                 const uint8_t *state,
                                 size_t state_len,
                                 const uint8_t *payloads,
                                 size_t payloads_len,
                                 uint8_t *out,
                                 size_t out_cap);

int32_t clausters_core_stats(const float *samples,
                             size_t n,
                             size_t channels,
                             size_t channel,
                             float *out);

float clausters_core_true_peak(const float *samples, size_t n, size_t channels, size_t channel);

int32_t clausters_core_loudness(const float *samples,
                                size_t n,
                                size_t channels,
                                double rate,
                                const double *weights,
                                double *out);

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_svg_to_display_list(const uint8_t *svg,
                                          size_t svg_len,
                                          uint8_t *out,
                                          size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_voice_to_mei(const uint8_t *voice,
                                   size_t voice_len,
                                   const uint8_t *meter,
                                   size_t meter_len,
                                   const uint8_t *clef,
                                   size_t clef_len,
                                   const uint8_t *key,
                                   size_t key_len,
                                   uint8_t *out,
                                   size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_voice_to_sheet(const uint8_t *voice,
                                     size_t voice_len,
                                     const uint8_t *meter,
                                     size_t meter_len,
                                     const uint8_t *clef,
                                     size_t clef_len,
                                     const uint8_t *key,
                                     size_t key_len,
                                     uint8_t *out,
                                     size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_sheet_apply(const uint8_t *sheet,
                                  size_t sheet_len,
                                  const uint8_t *op,
                                  size_t op_len,
                                  uint8_t *out,
                                  size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_sheet_to_mei(const uint8_t *sheet,
                                   size_t sheet_len,
                                   uint8_t *out,
                                   size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_sheet_ops(uint8_t *out, size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_mei_to_sheet(const uint8_t *mei,
                                   size_t mei_len,
                                   uint8_t *out,
                                   size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_sheet_perform(const uint8_t *sheet,
                                    size_t sheet_len,
                                    const uint8_t *interp,
                                    size_t interp_len,
                                    uint8_t *out,
                                    size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_sheet_render_events(const uint8_t *sheet,
                                          size_t sheet_len,
                                          const uint8_t *interp,
                                          size_t interp_len,
                                          uint8_t *out,
                                          size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_sheet_read_events(const uint8_t *sequence,
                                        size_t sequence_len,
                                        const uint8_t *how,
                                        size_t how_len,
                                        const uint8_t *interp,
                                        size_t interp_len,
                                        uint8_t *out,
                                        size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
size_t clausters_core_interpretation(uint8_t *out, size_t out_cap);
#endif

#if defined(CLAUSTERS_FFI_NOTATION)
int64_t clausters_core_item_id(const uint8_t *id, size_t id_len);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
struct ScoreHandle *clausters_score_open(const uint8_t *data,
                                         size_t data_len,
                                         int32_t scale,
                                         int32_t page_width,
                                         const uint8_t *options,
                                         size_t options_len);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
void clausters_score_free(struct ScoreHandle *h);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
size_t clausters_score_display_list(struct ScoreHandle *h,
                                    int32_t page,
                                    uint8_t *out,
                                    size_t out_cap);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
size_t clausters_score_mei(struct ScoreHandle *h, uint8_t *out, size_t out_cap);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_transpose(struct ScoreHandle *h,
                                  const uint8_t *element_id,
                                  size_t id_len,
                                  int32_t steps);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_transpose_to(struct ScoreHandle *h,
                                     const uint8_t *element_id,
                                     size_t id_len,
                                     int32_t position,
                                     int32_t page);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_apply(struct ScoreHandle *h, const uint8_t *op, size_t op_len);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
size_t clausters_score_sheet(struct ScoreHandle *h, uint8_t *out, size_t out_cap);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_edit(struct ScoreHandle *h,
                             const uint8_t *action,
                             size_t action_len,
                             const uint8_t *param,
                             size_t param_len);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_load(struct ScoreHandle *h, const uint8_t *mei, size_t mei_len);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_undo(struct ScoreHandle *h);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_redo(struct ScoreHandle *h);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_can_undo(struct ScoreHandle *h);
#endif

#if (defined(CLAUSTERS_FFI_NOTATION) && defined(CLAUSTERS_FFI_VEROVIO))
int32_t clausters_score_can_redo(struct ScoreHandle *h);
#endif

size_t clausters_core_patch_compile(const uint8_t *patch,
                                    size_t patch_len,
                                    uint8_t *out,
                                    size_t out_cap);

size_t clausters_core_plugin_catalog(const uint8_t *request,
                                     size_t len,
                                     uint8_t *out,
                                     size_t out_cap);

size_t clausters_core_plugin_search(const uint8_t *request,
                                    size_t len,
                                    uint8_t *out,
                                    size_t out_cap);

size_t clausters_core_plugin_reference(const uint8_t *request,
                                       size_t len,
                                       uint8_t *out,
                                       size_t out_cap);

size_t clausters_core_node_editor(const uint8_t *request, size_t len, uint8_t *out, size_t out_cap);

struct FfiRegistry *clausters_registry_new(int64_t base, uint64_t capacity);

void clausters_registry_free(struct FfiRegistry *h);

int64_t clausters_registry_alloc(struct FfiRegistry *h, uint64_t width);

int32_t clausters_registry_release(struct FfiRegistry *h, int64_t first, uint64_t width);

uint64_t clausters_registry_in_use(struct FfiRegistry *h);

uint64_t clausters_registry_capacity(struct FfiRegistry *h);

int32_t clausters_registry_contains(struct FfiRegistry *h, int64_t id);

void clausters_registry_clear(struct FfiRegistry *h);

int32_t clausters_registry_node_partition(uint64_t max_nodes, int64_t *out);

uint64_t clausters_registry_graph_audio_reserved(size_t audio_buses);

uint64_t clausters_registry_graph_control_reserved(size_t control_buses);

struct FfiIdSpaces *clausters_ids_new(uint64_t max_nodes,
                                      uint64_t audio_buses,
                                      uint64_t outputs,
                                      uint64_t control_buses,
                                      uint64_t buffers,
                                      uint64_t transports,
                                      uint64_t event_buses,
                                      uint32_t index,
                                      uint32_t of,
                                      int32_t score);

void clausters_ids_free(struct FfiIdSpaces *h);

int64_t clausters_ids_alloc(struct FfiIdSpaces *h, int32_t space, uint64_t width);

int32_t clausters_ids_release(struct FfiIdSpaces *h, int32_t space, int64_t first, uint64_t width);

int32_t clausters_ids_node_ended(struct FfiIdSpaces *h, int64_t node);

int32_t clausters_ids_contains(struct FfiIdSpaces *h, int32_t space, int64_t id);

uint64_t clausters_ids_in_use(struct FfiIdSpaces *h, int32_t space);

int32_t clausters_ids_narrow(struct FfiIdSpaces *h, uint32_t index, uint32_t of);

int32_t clausters_ids_share_of(int64_t base,
                               uint64_t span,
                               uint32_t index,
                               uint32_t of,
                               int64_t *out);

uint64_t clausters_rng_seed(uint64_t seed);

double clausters_rng_next_f64(uint64_t *state);

uint64_t clausters_rng_next_below(uint64_t *state, uint64_t n);

uint64_t clausters_rng_next_u64(uint64_t *state);

double clausters_rng_draw(uint64_t *state, uint32_t draw, double a, double b);

uint64_t clausters_rng_windex(uint64_t *state, const double *weights, size_t n);

double clausters_rng_tablerand(uint64_t *state, const double *table, size_t n);

size_t clausters_rng_sputter(uint64_t *state,
                             size_t n,
                             double probability,
                             size_t maxlen,
                             uint32_t *out);

void clausters_rng_scramble(uint64_t *state, uint32_t *out, size_t n);

double clausters_core_hz_to_mel(double hz);

double clausters_core_mel_to_hz(double mel);

double clausters_core_hz_to_bark(double hz);

double clausters_core_bark_to_hz(double bark);

struct Scheduler *clausters_sched_new(void);

void clausters_sched_free(struct Scheduler *h);

void clausters_sched_push(struct Scheduler *h, double time, uint64_t id);

int32_t clausters_sched_peek_time(struct Scheduler *h, double *out_time);

int32_t clausters_sched_pop_due(struct Scheduler *h,
                                double now,
                                double *out_time,
                                uint64_t *out_id);

size_t clausters_sched_remove(struct Scheduler *h, uint64_t id);

size_t clausters_sched_len(struct Scheduler *h);

void clausters_sched_clear(struct Scheduler *h);

uint32_t clausters_core_shm_abi_version(void);

int32_t clausters_core_shm_shape(const uint8_t *base, size_t len, struct Shape *out);

size_t clausters_core_shm_segment_size(size_t control_buses,
                                       size_t audio_buses,
                                       size_t taps,
                                       size_t tap_frames,
                                       size_t buffers);

int32_t clausters_core_shm_init(uint8_t *base,
                                size_t len,
                                size_t control_buses,
                                size_t audio_buses,
                                size_t taps,
                                size_t tap_frames);

int32_t clausters_core_shm_buffer_info(const uint8_t *base,
                                       size_t len,
                                       size_t bufnum,
                                       uint64_t *out_generation,
                                       uint64_t *out_frames,
                                       uint64_t *out_channels,
                                       double *out_sample_rate);

int32_t clausters_core_shm_region_suffix(size_t bufnum,
                                         uint64_t generation,
                                         uint8_t *out,
                                         size_t cap);

int32_t clausters_core_shm_push(const uint8_t *base,
                                size_t len,
                                uint32_t role,
                                uint32_t peer,
                                const uint8_t *packet,
                                size_t packet_len);

int32_t clausters_core_shm_pop(const uint8_t *base,
                               size_t len,
                               uint32_t role,
                               uint8_t *out,
                               size_t cap,
                               uint32_t *out_peer,
                               size_t *out_len);

struct TempoMap *clausters_tempomap_new(double tempo);

uint64_t clausters_tempomap_version(const struct TempoMap *h);

size_t clausters_tempomap_dump(const struct TempoMap *h, uint8_t *out, size_t out_cap);

struct TempoMap *clausters_tempomap_load(const uint8_t *json, size_t len);

struct TempoMap *clausters_tempomap_from_changes(const uint8_t *json,
                                                 size_t len,
                                                 double default_tempo);

struct TempoMap *clausters_tempomap_anchored(double tempo, double base_beats, double base_seconds);

void clausters_tempomap_free(struct TempoMap *h);

struct TempoMap *clausters_tempomap_clone(const struct TempoMap *h);

double clausters_tempomap_secs_at(const struct TempoMap *h, double b);

double clausters_tempomap_beats_at(const struct TempoMap *h, double s);

double clausters_tempomap_tempo_at(const struct TempoMap *h, double b);

double clausters_tempomap_span_secs(const struct TempoMap *h, double b0, double b1);

double clausters_tempomap_span_beats(const struct TempoMap *h, double b0, double secs);

int32_t clausters_tempomap_push(struct TempoMap *h, double b, double tempo);

int32_t clausters_tempomap_ramp(struct TempoMap *h,
                                double from_beats,
                                double to_beats,
                                double from_tempo,
                                double to_tempo);

int32_t clausters_tempomap_shaped(struct TempoMap *h,
                                  double from_beats,
                                  double to_beats,
                                  double from_tempo,
                                  double to_tempo,
                                  uint32_t shape,
                                  double curvature);

int32_t clausters_tempomap_env(struct TempoMap *h,
                               double at,
                               const double *tempos,
                               const double *extents,
                               const uint32_t *shapes,
                               const double *curvatures,
                               uint32_t n,
                               int32_t seconds);

void clausters_tempomap_truncate_from(struct TempoMap *h, double b);

uint32_t clausters_tempomap_len(const struct TempoMap *h);

int32_t clausters_tempomap_segment(const struct TempoMap *h, uint32_t i, double *out);

int32_t clausters_tempomap_last(const struct TempoMap *h, double *out);

double clausters_core_beats_to_secs(double tempo,
                                    double base_beats,
                                    double base_seconds,
                                    double beats);

double clausters_core_secs_to_beats(double tempo,
                                    double base_beats,
                                    double base_seconds,
                                    double secs);

int64_t clausters_core_secs_to_samples(double secs, double sample_rate);

double clausters_core_samples_to_secs(int64_t samples, double sample_rate);

int64_t clausters_core_unix_to_sample(double unix_secs,
                                      double anchor_unix,
                                      int64_t anchor_sample,
                                      double sample_rate);

double clausters_core_quant_delay(double pos, double quant);

double clausters_core_bar(double beats, double quant);

double clausters_core_beat_in_bar(double beats, double quant);

uint64_t clausters_core_ntp_timetag(double ntp_secs);

uint64_t clausters_core_unix_to_ntp(double unix_secs);

struct FfiWidgetIds *clausters_widgetids_new(int64_t base, uint64_t capacity);

void clausters_widgetids_free(struct FfiWidgetIds *h);

int64_t clausters_widgetids_owner(struct FfiWidgetIds *h);

int64_t clausters_widgetids_alloc(struct FfiWidgetIds *h);

void clausters_widgetids_release(struct FfiWidgetIds *h, int64_t id);

int64_t clausters_widgetids_id_for(struct FfiWidgetIds *h,
                                   int64_t owner,
                                   int64_t structure,
                                   const uint8_t *role,
                                   size_t role_len,
                                   const uint8_t *key,
                                   size_t key_len);

int64_t clausters_widgetids_id_of(struct FfiWidgetIds *h,
                                  int64_t structure,
                                  const uint8_t *role,
                                  size_t role_len,
                                  const uint8_t *key,
                                  size_t key_len);

int64_t clausters_widgetids_forget(struct FfiWidgetIds *h,
                                   int64_t structure,
                                   const uint8_t *role,
                                   size_t role_len,
                                   const uint8_t *key,
                                   size_t key_len);

void clausters_widgetids_begin(struct FfiWidgetIds *h, int64_t owner);

int64_t clausters_widgetids_retire(struct FfiWidgetIds *h, int64_t owner, int64_t *out, size_t cap);

uint64_t clausters_widgetids_in_use(struct FfiWidgetIds *h);

uint64_t clausters_widgetids_named(struct FfiWidgetIds *h);

int32_t clausters_widgetids_contains(struct FfiWidgetIds *h, int64_t id);

void clausters_widgetids_clear(struct FfiWidgetIds *h);

struct WsClient *clausters_ws_connect(const char *host, uint16_t port, const char *path);

int32_t clausters_ws_send(struct WsClient *handle, const uint8_t *ptr, size_t len);

ptrdiff_t clausters_ws_recv(struct WsClient *handle, uint8_t *buf, size_t cap, uint32_t timeout_ms);

void clausters_ws_close(struct WsClient *handle);

const char *clausters_ws_last_error(void);

#endif  /* CLAUSTERS_FFI_H */
