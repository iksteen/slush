/* Independent test oracle; never linked into the Rust application. */
#define _GNU_SOURCE
#include "osc5577.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void hex(const char *s) {
  for (; *s; s++) printf("%02x", (unsigned char)*s);
}
int main(void) {
  char *line = NULL;
  size_t cap = 0;
  ssize_t n;
  while ((n = getline(&line, &cap, stdin)) > 0) {
    unsigned char bytes[8192];
    size_t len = 0;
    for (size_t i = 0; i + 1 < (size_t)n && len < sizeof bytes; i += 2) {
      unsigned v;
      if (sscanf(line + i, "%2x", &v) != 1) break;
      bytes[len++] = (unsigned char)v;
    }
    osc7501_t r;
    if (!osc7501_parse((const char *)bytes, len, &r)) { puts("invalid"); continue; }
    printf("%s|%s|%d|", osc7501_state_name(r.state), osc7501_kind_name(r.kind), r.progress);
    hex(r.id); putchar('|'); hex(r.app); putchar('|'); hex(r.title); putchar('|'); hex(r.msg); putchar('\n');
  }
  free(line);
}
