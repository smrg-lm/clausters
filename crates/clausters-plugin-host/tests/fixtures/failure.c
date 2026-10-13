/* Separate binaries fail during entry initialization, before any instance exists. */
#include <clap/entry.h>
#include <signal.h>
#include <unistd.h>
static bool init(const char *path) {
    (void)path;
#if FAILURE_MODE == 1
    raise(SIGKILL);
#elif FAILURE_MODE == 2
    for (;;) pause();
#endif
    return true;
}
static void deinit(void) {}
static const void *factory(const char *id) { (void)id; return NULL; }
CLAP_EXPORT const clap_plugin_entry_t clap_entry = {
#if FAILURE_MODE == 3
    {0, 99, 0},
#else
    CLAP_VERSION_INIT,
#endif
    init, deinit, factory
};
