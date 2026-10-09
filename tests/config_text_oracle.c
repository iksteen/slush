/* Test only: report the original loader's fixed text buffers as hex. */
#include "config.h"
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
  if (argc != 2) return 2;
  config_t c;
  char error[192] = {0};
  config_defaults(&c);
  if (!config_load(&c, argv[1], error, sizeof error)) {
    fprintf(stderr, "%s\n", error);
    config_free(&c);
    return 1;
  }
  const char *values[] = {c.zoom_mark, c.zoom_on_mark,   c.close_mark,
                          c.min_mark,  c.newtab_mark,    c.bell_mark,
                          c.busy_mark, c.word_separators};
  for (size_t i = 0; i < sizeof values / sizeof *values; i++) {
    for (size_t n = 0; n < strlen(values[i]); n++)
      printf("%02x", (unsigned char)values[i][n]);
    putchar('\n');
  }
  config_free(&c);
  return 0;
}
