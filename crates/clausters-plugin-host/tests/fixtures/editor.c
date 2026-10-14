/* A real X11 CLAP editor: timers, fd readiness and editor-to-audio edits. */
#include <clap/plugin.h>
#include <clap/factory/plugin-factory.h>
#define clap_entry base_entry
#define create base_create
#define extension base_extension
#define process base_process
#include "clap.c"
#undef clap_entry
#undef create
#undef extension
#undef process
#include <clap/ext/gui.h>
#include <clap/ext/timer-support.h>
#include <clap/ext/posix-fd-support.h>
#include <clap/ext/thread-check.h>
#include <X11/Xlib.h>
#include <assert.h>

typedef struct {
    const clap_host_t *host;
    Display *display;
    Window child;
    clap_id timer;
    int pipefd[2];
    uint32_t width, height;
    _Atomic unsigned edit;
} Editor;
/* One instance per isolated process, plus probe/discovery instances. */
static Editor editor;
static void main_only(void) {
    const clap_host_thread_check_t *check = editor.host->get_extension(editor.host, CLAP_EXT_THREAD_CHECK);
    assert(check && check->is_main_thread(editor.host) && !check->is_audio_thread(editor.host));
}
static bool supported(const clap_plugin_t *p, const char *api, bool floating) {
    (void)p; return !floating && !strcmp(api, CLAP_WINDOW_API_X11);
}
static bool preferred(const clap_plugin_t *p, const char **api, bool *floating) {
    (void)p; *api = CLAP_WINDOW_API_X11; *floating = false; return true;
}
static bool gui_create(const clap_plugin_t *p, const char *api, bool floating) {
    (void)p; main_only();
    if (!supported(p, api, floating)) return false;
    editor.display = XOpenDisplay(NULL);
    if (!editor.display) return false;
    editor.width = 320; editor.height = 200;
    return true;
}
static void gui_free(const clap_plugin_t *p) {
    (void)p; main_only();
    const clap_host_timer_support_t *timer = editor.host->get_extension(editor.host, CLAP_EXT_TIMER_SUPPORT);
    const clap_host_posix_fd_support_t *fd = editor.host->get_extension(editor.host, CLAP_EXT_POSIX_FD_SUPPORT);
    if (editor.child) {
        assert(timer->unregister_timer(editor.host, editor.timer));
        assert(fd->unregister_fd(editor.host, editor.pipefd[0]));
        close(editor.pipefd[0]); close(editor.pipefd[1]);
        XDestroyWindow(editor.display, editor.child); editor.child = 0;
    }
    if (editor.display) { XCloseDisplay(editor.display); editor.display = NULL; }
}
static bool scale(const clap_plugin_t *p, double value) { (void)p; return value > 0; }
static bool size(const clap_plugin_t *p, uint32_t *w, uint32_t *h) { (void)p; *w = editor.width; *h = editor.height; return true; }
static bool resizable(const clap_plugin_t *p) { (void)p; return true; }
static bool hints(const clap_plugin_t *p, clap_gui_resize_hints_t *h) {
    (void)p; *h = (clap_gui_resize_hints_t){.can_resize_horizontally=true, .can_resize_vertically=true}; return true;
}
static bool adjust(const clap_plugin_t *p, uint32_t *w, uint32_t *h) { (void)p; return *w >= 100 && *h >= 100; }
static bool set_size(const clap_plugin_t *p, uint32_t w, uint32_t h) {
    (void)p; main_only(); editor.width = w; editor.height = h;
    if (editor.child) { XResizeWindow(editor.display, editor.child, w, h); XFlush(editor.display); }
    return true;
}
static bool parent(const clap_plugin_t *p, const clap_window_t *window) {
    (void)p; main_only();
    if (strcmp(window->api, CLAP_WINDOW_API_X11)) return false;
    editor.child = XCreateSimpleWindow(editor.display, window->x11, 0, 0, editor.width, editor.height, 0, 0, 0x224466);
    XSelectInput(editor.display, editor.child, ExposureMask | ButtonPressMask);
    assert(pipe(editor.pipefd) == 0);
    const clap_host_timer_support_t *timer = editor.host->get_extension(editor.host, CLAP_EXT_TIMER_SUPPORT);
    const clap_host_posix_fd_support_t *fd = editor.host->get_extension(editor.host, CLAP_EXT_POSIX_FD_SUPPORT);
    assert(timer && fd && timer->register_timer(editor.host, 20, &editor.timer));
    assert(fd->register_fd(editor.host, editor.pipefd[0], CLAP_POSIX_FD_READ));
    assert(write(editor.pipefd[1], "x", 1) == 1);
    return true;
}
static void title(const clap_plugin_t *p, const char *text) { (void)p; (void)text; }
static bool show(const clap_plugin_t *p) { (void)p; main_only(); XMapWindow(editor.display, editor.child); XFlush(editor.display); return true; }
static bool hide(const clap_plugin_t *p) { (void)p; main_only(); XUnmapWindow(editor.display, editor.child); XFlush(editor.display); return true; }
static const clap_plugin_gui_t gui = {supported, preferred, gui_create, gui_free, scale, size, resizable, hints, adjust, set_size, parent, parent, title, show, hide};
static void on_timer(const clap_plugin_t *p, clap_id id) {
    (void)p; main_only(); assert(id == editor.timer);
    while (XPending(editor.display)) {
        XEvent e; XNextEvent(editor.display, &e);
        if (e.type == ButtonPress) {
            atomic_store(&editor.edit, 1);
            const clap_host_params_t *host_params = editor.host->get_extension(editor.host, CLAP_EXT_PARAMS);
            assert(host_params); host_params->request_flush(editor.host);
        }
        if (e.type == Expose) {
            GC gc = XCreateGC(editor.display, editor.child, 0, NULL);
            XSetForeground(editor.display, gc, 0xffffff);
            const char *label = "Clausters CLAP fixture - click to edit gain";
            XDrawString(editor.display, editor.child, gc, 12, 30, label, (int)strlen(label));
            XFreeGC(editor.display, gc); XFlush(editor.display);
        }
    }
}
static void on_fd(const clap_plugin_t *p, int fd, clap_posix_fd_flags_t flags) {
    (void)p; main_only(); assert(fd == editor.pipefd[0] && flags & CLAP_POSIX_FD_READ);
    char byte; assert(read(fd, &byte, 1) == 1);
    const clap_host_gui_t *host_gui = editor.host->get_extension(editor.host, CLAP_EXT_GUI);
    set_size(p, 360, 240);
    assert(host_gui && host_gui->request_resize(editor.host, 360, 240));
    atomic_store(&editor.edit, 1);
    const clap_host_params_t *host_params = editor.host->get_extension(editor.host, CLAP_EXT_PARAMS);
    assert(host_params); host_params->request_flush(editor.host);
}
static const clap_plugin_timer_support_t timers = {on_timer};
static const clap_plugin_posix_fd_support_t fds = {on_fd};
static void editor_flush(const clap_plugin_t *p, const clap_input_events_t *input, const clap_output_events_t *output) {
    param_flush(p, input, output);
    if (atomic_exchange(&editor.edit, 0)) {
        data(p)->gain = 0.75;
        clap_event_param_gesture_t gesture = {.header={sizeof(gesture), 0, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_GESTURE_BEGIN, 0}, .param_id=0};
        assert(output->try_push(output, &gesture.header));
        clap_event_param_value_t value = {.header={sizeof(value), 0, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, 0}, .param_id=0, .note_id=-1, .port_index=-1, .channel=-1, .key=-1, .value=0.75};
        assert(output->try_push(output, &value.header));
        gesture.header.type = CLAP_EVENT_PARAM_GESTURE_END;
        assert(output->try_push(output, &gesture.header));
    }
}
static clap_process_status process(const clap_plugin_t *p, const clap_process_t *ctx) {
    editor_flush(p, ctx->in_events, ctx->out_events);
    return base_process(p, ctx);
}
static const clap_plugin_params_t editor_params = {param_count, param_info, param_value, param_text, param_parse, editor_flush};
static const void *extension(const clap_plugin_t *p, const char *id) {
    if (!strcmp(id, CLAP_EXT_PARAMS)) return &editor_params;
    if (!strcmp(id, CLAP_EXT_GUI)) return &gui;
    if (!strcmp(id, CLAP_EXT_TIMER_SUPPORT)) return &timers;
    if (!strcmp(id, CLAP_EXT_POSIX_FD_SUPPORT)) return &fds;
    return base_extension(p, id);
}
/* The original factory captures base_create; install this wrapper at entry init. */
static const clap_plugin_t *editor_create(const clap_plugin_factory_t *f, const clap_host_t *host, const char *id) {
    const clap_plugin_t *p = base_create(f, host, id);
    if (p) { editor.host = host; ((clap_plugin_t *)p)->get_extension = extension; ((clap_plugin_t *)p)->process = process; }
    return p;
}
static const clap_plugin_factory_t editor_factory = {count, descriptor, editor_create};
static const void *editor_factory_get(const char *id) { return !strcmp(id, CLAP_PLUGIN_FACTORY_ID) ? &editor_factory : NULL; }
CLAP_EXPORT const clap_plugin_entry_t clap_entry = {CLAP_VERSION_INIT, entry_init, entry_deinit, editor_factory_get};
