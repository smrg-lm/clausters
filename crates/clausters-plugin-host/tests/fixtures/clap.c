/* Deterministic CLAP fixtures. No dependency on the server or audio hardware. */
#include <clap/entry.h>
#include <clap/factory/plugin-factory.h>
#include <clap/host.h>
#include <clap/plugin.h>
#include <clap/ext/audio-ports.h>
#include <clap/ext/note-ports.h>
#include <clap/ext/latency.h>
#include <clap/ext/params.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
#include <stdatomic.h>

typedef struct {
    clap_plugin_t api;
    unsigned kind, cursor;
    _Atomic double gain;
    bool held;
    float delay[7];
} Fixture;

static const char *const fx_features[] = {"audio-effect", NULL};
static const char *const instrument_features[] = {"instrument", NULL};
#define DESC(id, features) {CLAP_VERSION_INIT, "org.clausters.fixture." id, id, \
    "Clausters", "", "", "", "1", "Bridge test fixture", features}
static const clap_plugin_descriptor_t descriptors[] = {
    DESC("gain", fx_features), DESC("delay", fx_features),
    DESC("instrument", instrument_features), DESC("crash", fx_features),
    DESC("hang", fx_features), DESC("error", fx_features)
};
static Fixture *data(const clap_plugin_t *p) { return p->plugin_data; }
static bool init(const clap_plugin_t *p) { (void)p; return true; }
static void destroy(const clap_plugin_t *p) { free(data(p)); }
static bool activate(const clap_plugin_t *p, double rate, uint32_t min, uint32_t max) {
    (void)p; return rate > 0 && min > 0 && max <= 64;
}
static void noop(const clap_plugin_t *p) { (void)p; }
static void reset(const clap_plugin_t *p) {
    Fixture *f = data(p); f->held = false; f->cursor = 0;
    memset(f->delay, 0, sizeof(f->delay)); f->gain = 0.5;
}
static clap_process_status process(const clap_plugin_t *p, const clap_process_t *ctx) {
    Fixture *f = data(p);
    if (ctx->steady_time >= 128) {
        if (f->kind == 3) raise(SIGKILL);
        if (f->kind == 4) for (;;) pause();
        if (f->kind == 5) return CLAP_PROCESS_ERROR;
    }
    uint32_t event = 0, count = ctx->in_events->size(ctx->in_events);
    for (uint32_t i = 0; i < ctx->frames_count; ++i) {
        while (event < count) {
            const clap_event_header_t *h = ctx->in_events->get(ctx->in_events, event);
            if (h->time != i) break;
            if (h->space_id == CLAP_CORE_EVENT_SPACE_ID) {
                if (h->type == CLAP_EVENT_NOTE_ON) f->held = true;
                if (h->type == CLAP_EVENT_NOTE_OFF || h->type == CLAP_EVENT_NOTE_CHOKE)
                    f->held = false;
                if (h->type == CLAP_EVENT_PARAM_VALUE)
                    f->gain = ((const clap_event_param_value_t *)h)->value;
            }
            ++event;
        }
        float x = ctx->audio_inputs_count ? ctx->audio_inputs[0].data32[0][i] : 0;
        float y = x * (float)f->gain;
        if (f->kind == 1) {
            y = f->delay[f->cursor]; f->delay[f->cursor] = x;
            f->cursor = (f->cursor + 1) % 7;
        }
        if (f->kind == 2) {
            /* Position is deliberately audible in the numeric fixture. */
            bool playing = ctx->transport && (ctx->transport->flags & CLAP_TRANSPORT_IS_PLAYING);
            y = playing && f->held ?
                0.25f + (float)((double)ctx->transport->song_pos_seconds / CLAP_SECTIME_FACTOR) : 0.0f;
        }
        ctx->audio_outputs[0].data32[0][i] = y;
    }
    return CLAP_PROCESS_CONTINUE;
}
static uint32_t port_count(const clap_plugin_t *p, bool input) {
    return input && data(p)->kind == 2 ? 0 : 1;
}
static bool port_get(const clap_plugin_t *p, uint32_t index, bool input,
                     clap_audio_port_info_t *info) {
    if (index >= port_count(p, input)) return false;
    *info = (clap_audio_port_info_t){.id = 0, .flags = CLAP_AUDIO_PORT_IS_MAIN,
        .channel_count = 1, .port_type = CLAP_PORT_MONO, .in_place_pair = CLAP_INVALID_ID};
    strcpy(info->name, input ? "Input" : "Output"); return true;
}
static uint32_t note_count(const clap_plugin_t *p, bool input) {
    return input && data(p)->kind == 2 ? 1 : 0;
}
static bool note_get(const clap_plugin_t *p, uint32_t index, bool input,
                     clap_note_port_info_t *info) {
    if (index >= note_count(p, input)) return false;
    *info = (clap_note_port_info_t){.id = 0, .supported_dialects = CLAP_NOTE_DIALECT_CLAP,
        .preferred_dialect = CLAP_NOTE_DIALECT_CLAP}; strcpy(info->name, "Notes"); return true;
}
static uint32_t latency_get(const clap_plugin_t *p) { return data(p)->kind == 1 ? 7 : 0; }
static const clap_plugin_audio_ports_t audio_ports = {port_count, port_get};
static const clap_plugin_note_ports_t note_ports = {note_count, note_get};
static const clap_plugin_latency_t latency = {latency_get};
static uint32_t param_count(const clap_plugin_t *p) { return data(p)->kind == 0 ? 1 : 0; }
static bool param_info(const clap_plugin_t *p, uint32_t index, clap_param_info_t *info) {
    if (index >= param_count(p)) return false;
    *info = (clap_param_info_t){.id = 0, .flags = CLAP_PARAM_IS_AUTOMATABLE,
        .min_value = 0, .max_value = 1, .default_value = 0.5};
    strcpy(info->name, "Gain"); return true;
}
static bool param_value(const clap_plugin_t *p, clap_id id, double *value) {
    if (id || !param_count(p)) return false;
    *value = data(p)->gain; return true;
}
static bool param_text(const clap_plugin_t *p, clap_id id, double value, char *text, uint32_t size) {
    if (id || !param_count(p) || !size) return false;
    snprintf(text, size, "%.3f", value); return true;
}
static bool param_parse(const clap_plugin_t *p, clap_id id, const char *text, double *value) {
    if (id || !param_count(p)) return false;
    char *end; *value = strtod(text, &end); return end != text && !*end;
}
static void param_flush(const clap_plugin_t *p, const clap_input_events_t *input,
                         const clap_output_events_t *output) {
    (void)output;
    for (uint32_t i = 0; i < input->size(input); ++i) {
        const clap_event_header_t *h = input->get(input, i);
        if (h->space_id == CLAP_CORE_EVENT_SPACE_ID && h->type == CLAP_EVENT_PARAM_VALUE)
            data(p)->gain = ((const clap_event_param_value_t *)h)->value;
    }
}
static const clap_plugin_params_t params = {
    param_count, param_info, param_value, param_text, param_parse, param_flush
};
static const void *extension(const clap_plugin_t *p, const char *id) {
    (void)p;
    if (!strcmp(id, CLAP_EXT_AUDIO_PORTS)) return &audio_ports;
    if (!strcmp(id, CLAP_EXT_NOTE_PORTS)) return &note_ports;
    if (!strcmp(id, CLAP_EXT_LATENCY)) return &latency;
    if (!strcmp(id, CLAP_EXT_PARAMS)) return &params;
    return NULL;
}
static uint32_t count(const clap_plugin_factory_t *f) { (void)f; return 6; }
static const clap_plugin_descriptor_t *descriptor(const clap_plugin_factory_t *f, uint32_t i) {
    (void)f; return i < 6 ? &descriptors[i] : NULL;
}
static const clap_plugin_t *create(const clap_plugin_factory_t *factory,
                                  const clap_host_t *host, const char *id) {
    (void)factory;
    if (!clap_version_is_compatible(host->clap_version)) return NULL;
    for (unsigned i = 0; i < 6; ++i) if (!strcmp(id, descriptors[i].id)) {
        Fixture *f = calloc(1, sizeof(*f)); if (!f) return NULL;
        f->kind = i; f->gain = 0.5;
        f->api = (clap_plugin_t){&descriptors[i], f, init, destroy, activate,
            noop, init, noop, reset, process, extension, noop};
        return &f->api;
    }
    return NULL;
}
static const clap_plugin_factory_t factory = {count, descriptor, create};
static bool entry_init(const char *path) { (void)path; return true; }
static void entry_deinit(void) {}
static const void *get_factory(const char *id) {
    return !strcmp(id, CLAP_PLUGIN_FACTORY_ID) ? &factory : NULL;
}
CLAP_EXPORT const clap_plugin_entry_t clap_entry = {
    CLAP_VERSION_INIT, entry_init, entry_deinit, get_factory
};
