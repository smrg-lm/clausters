/* Deterministic persistent instrument: native identities and transport are audible numerically. */
#define clap_entry base_clap_entry
#include "clap.c"
#undef clap_entry
#include <assert.h>
#include <clap/events.h>

typedef struct Held { bool active; int32_t id; int16_t channel, key; double value, pressure, tuning; } Held;
static Held held[256];
static double volume = 1;
static int64_t previous_end = -1;
static bool poly_note_get(const clap_plugin_t *p, uint32_t index, bool input, clap_note_port_info_t *info) {
    if (!note_get(p, index, input, info)) return false;
    info->supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    return true;
}
static const clap_plugin_note_ports_t poly_notes = { note_count, poly_note_get };
static const void *poly_extension(const clap_plugin_t *p, const char *id) {
    if (!strcmp(id, CLAP_EXT_NOTE_PORTS)) return &poly_notes;
    return extension(p, id);
}
static bool match_note(Held h, int32_t id, int16_t channel, int16_t key) {
    return h.active && (id == -1 || h.id == id) && (channel == -1 || h.channel == channel) && (key == -1 || h.key == key);
}
static clap_process_status poly_process(const clap_plugin_t *p, const clap_process_t *ctx) {
    (void)p;
    assert(ctx->transport);
    assert(previous_end <= ctx->steady_time);
    previous_end = ctx->steady_time + ctx->frames_count;
    assert(ctx->transport->flags & CLAP_TRANSPORT_HAS_TEMPO);
    assert(ctx->transport->flags & CLAP_TRANSPORT_HAS_SECONDS_TIMELINE);
    assert(ctx->transport->flags & CLAP_TRANSPORT_HAS_BEATS_TIMELINE);
    uint32_t at = 0, count = ctx->in_events->size(ctx->in_events);
    for (uint32_t i = 0; i < ctx->frames_count; ++i) {
        while (at < count) {
            const clap_event_header_t *event = ctx->in_events->get(ctx->in_events, at);
            if (event->time > i) break;
            assert(event->time == i);
            if (event->type == CLAP_EVENT_NOTE_ON) {
                const clap_event_note_t *note = (const clap_event_note_t *)event;
                unsigned j;
                for (j = 0; j < 256; ++j) if (!held[j].active) break;
                assert(j < 256);
                held[j] = (Held){.active=true, .id=note->note_id, .channel=note->channel, .key=note->key, .value=note->velocity};
            } else if (event->type == CLAP_EVENT_NOTE_OFF || event->type == CLAP_EVENT_NOTE_CHOKE) {
                const clap_event_note_t *note = (const clap_event_note_t *)event;
                for (unsigned j = 0; j < 256; ++j) if (match_note(held[j], note->note_id, note->channel, note->key)) held[j].active = false;
            } else if (event->type == CLAP_EVENT_NOTE_EXPRESSION) {
                const clap_event_note_expression_t *expression = (const clap_event_note_expression_t *)event;
                for (unsigned j = 0; j < 256; ++j) if (match_note(held[j], expression->note_id, expression->channel, expression->key)) {
                    if (expression->expression_id == CLAP_NOTE_EXPRESSION_PRESSURE) held[j].pressure = expression->value;
                    if (expression->expression_id == CLAP_NOTE_EXPRESSION_TUNING) held[j].tuning = expression->value;
                }
            } else if (event->type == CLAP_EVENT_MIDI) {
                const clap_event_midi_t *midi = (const clap_event_midi_t *)event;
                if ((midi->data[0]&0xf0) == 0xb0 && midi->data[1] == 7) volume = midi->data[2]/127.0;
            }
            ++at;
        }
        double sum = 0;
        for (unsigned j = 0; j < 256; ++j) if (held[j].active) sum += held[j].value + held[j].pressure * 0.1 + held[j].tuning * 0.01;
        double seconds = (double)ctx->transport->song_pos_seconds / CLAP_SECTIME_FACTOR;
        double y = sum == 0 ? 0 : volume * sum + ctx->transport->tempo/10000.0 + seconds*0.01;
        ctx->audio_outputs[0].data32[0][i] = (float)y;
    }
    return CLAP_PROCESS_CONTINUE;
}
static const clap_plugin_t *poly_create(const clap_plugin_factory_t *factory, const clap_host_t *host, const char *id) {
    const clap_plugin_t *base = create(factory, host, id);
    if (!base) return NULL;
    clap_plugin_t *p = &data(base)->api;
    p->get_extension = poly_extension; p->process = poly_process;
    memset(held, 0, sizeof(held)); volume = 1; previous_end = -1;
    return p;
}
static const clap_plugin_factory_t poly_factory = {count, descriptor, poly_create};
static const void *poly_get_factory(const char *id) { return !strcmp(id, CLAP_PLUGIN_FACTORY_ID) ? &poly_factory : NULL; }
CLAP_EXPORT const clap_plugin_entry_t clap_entry = {CLAP_VERSION_INIT, entry_init, entry_deinit, poly_get_factory};
