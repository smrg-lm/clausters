/* Reuse the deterministic DSP fixtures, adding state, event output and thread assertions. */
#define clap_entry base_clap_entry
#include "clap.c"
#undef clap_entry
#include <clap/ext/state.h>
#include <clap/ext/thread-check.h>
#include <assert.h>

static const clap_host_t *active_host;
static void check_thread(bool audio) {
    const clap_host_thread_check_t *check = active_host->get_extension(active_host, CLAP_EXT_THREAD_CHECK);
    assert(check);
    assert(check->is_audio_thread(active_host) == audio);
    assert(check->is_main_thread(active_host) != audio);
}
static bool save_state(const clap_plugin_t *p, const clap_ostream_t *stream) {
    check_thread(false);
    double value = data(p)->gain;
    return stream->write(stream, &value, sizeof(value)) == sizeof(value);
}
static bool load_state(const clap_plugin_t *p, const clap_istream_t *stream) {
    check_thread(false);
    double value;
    if (stream->read(stream, &value, sizeof(value)) != sizeof(value)) return false;
    data(p)->gain = value; return true;
}
static const clap_plugin_state_t state_extension = {save_state, load_state};
static uint32_t multi_count(const clap_plugin_t *p, bool input) { (void)p; (void)input; return 2; }
static bool multi_get(const clap_plugin_t *p, uint32_t index, bool input, clap_audio_port_info_t *info) {
    (void)p;
    if (index >= 2) return false;
    *info = (clap_audio_port_info_t){.id = (input ? 10 : 20) + index,
        .flags = index ? 0 : CLAP_AUDIO_PORT_IS_MAIN, .channel_count = index ? 1 : 2,
        .port_type = index ? CLAP_PORT_MONO : CLAP_PORT_STEREO, .in_place_pair = CLAP_INVALID_ID};
    strcpy(info->name, index ? "Additional" : "Main"); return true;
}
static const clap_plugin_audio_ports_t multi_ports = {multi_count, multi_get};
static uint32_t events_count(const clap_plugin_t *p, bool input) { (void)p; (void)input; return 1; }
static bool events_get(const clap_plugin_t *p, uint32_t index, bool input, clap_note_port_info_t *info) {
    return note_get(p, index, true, info) && (input || index == 0);
}
static const clap_plugin_note_ports_t event_ports = {events_count, events_get};
static const void *extended_extension(const clap_plugin_t *p, const char *id) {
    if (data(p)->kind == 2 && !strcmp(id, CLAP_EXT_NOTE_PORTS)) return &event_ports;
    if (!strcmp(p->desc->id, "org.clausters.fixture.ports") && !strcmp(id, CLAP_EXT_AUDIO_PORTS)) return &multi_ports;
    if (!strcmp(id, CLAP_EXT_STATE)) return &state_extension;
    return extension(p, id);
}
static bool checked_start(const clap_plugin_t *p) { (void)p; check_thread(true); return true; }
static void checked_stop(const clap_plugin_t *p) { (void)p; check_thread(true); }
static clap_process_status checked_process(const clap_plugin_t *p, const clap_process_t *ctx) {
    check_thread(true);
    if (!strcmp(p->desc->id, "org.clausters.fixture.ports")) {
        for (unsigned port = 0; port < ctx->audio_outputs_count; ++port)
            for (unsigned channel = 0; channel < ctx->audio_outputs[port].channel_count; ++channel)
                for (unsigned i = 0; i < ctx->frames_count; ++i)
                    ctx->audio_outputs[port].data32[channel][i] = ctx->audio_inputs[port].data32[channel][i] * (float)data(p)->gain;
        return CLAP_PROCESS_CONTINUE;
    }
    if (data(p)->kind == 2) {
        for (uint32_t i = 0; i < ctx->in_events->size(ctx->in_events); ++i)
            assert(ctx->out_events->try_push(ctx->out_events, ctx->in_events->get(ctx->in_events, i)));
    }
    return process(p, ctx);
}
static const clap_plugin_descriptor_t extra[] = {
    DESC("state", fx_features), DESC("events", instrument_features), DESC("ports", fx_features)
};
static uint32_t extra_count(const clap_plugin_factory_t *f) { (void)f; return 3; }
static const clap_plugin_descriptor_t *extra_descriptor(const clap_plugin_factory_t *f, uint32_t i) {
    (void)f; return i < 3 ? &extra[i] : NULL;
}
static const clap_plugin_t *extra_create(const clap_plugin_factory_t *f, const clap_host_t *host, const char *id) {
    for (unsigned i = 0; i < 3; ++i) if (!strcmp(id, extra[i].id)) {
        active_host = host;
        const clap_plugin_t *base = create(f, host, descriptors[i == 1 ? 2 : 0].id);
        if (!base) return NULL;
        clap_plugin_t *p = &data(base)->api;
        p->desc = &extra[i]; p->get_extension = extended_extension;
        p->start_processing = checked_start; p->stop_processing = checked_stop; p->process = checked_process;
        return p;
    }
    return NULL;
}
static const clap_plugin_factory_t extra_factory = {extra_count, extra_descriptor, extra_create};
static const void *extra_get_factory(const char *id) {
    return !strcmp(id, CLAP_PLUGIN_FACTORY_ID) ? &extra_factory : NULL;
}
CLAP_EXPORT const clap_plugin_entry_t clap_entry = {
    CLAP_VERSION_INIT, entry_init, entry_deinit, extra_get_factory
};
