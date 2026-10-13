/* An incompatible helper must be rejected and reaped before any load/scan. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) return 1;
    char path[4096];
    if (snprintf(path, sizeof(path), "%s.pid", argv[0]) >= (int)sizeof(path)) return 1;
    FILE *file = fopen(path, "w");
    if (!file) return 1;
    fprintf(file, "%ld", (long)getpid());
    fclose(file);
    const char *json = "{\"control\":4294967295,\"audio\":1,\"frames\":64,\"slots\":32,\"events\":16,\"spans\":8}";
    uint32_t n = (uint32_t)strlen(json);
    uint8_t length[4] = {n, n >> 8, n >> 16, n >> 24};
    int fd = atoi(argv[2]);
    if (write(fd, length, 4) != 4 || write(fd, json, n) != n) return 1;
    for (;;) pause();
}
