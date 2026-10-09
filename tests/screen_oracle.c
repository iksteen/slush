/* Test oracle only: exercise the original compositor with a scripted trace. */
#include "slosh.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
  screen_t screen;
  screen_init(&screen, 20, 8);
  char line[4096];
  while (fgets(line, sizeof line, stdin)) {
    unsigned x, y, a, b, attrs;
    int rgb;
    char hex[1024];
    if (sscanf(line, "size %u %u", &x, &y) == 2) {
      screen_resize(&screen, x, y);
    } else if (sscanf(line, "put %u %u %u %d %1023s", &x, &y, &attrs, &rgb, hex) == 5) {
      char bytes[512];
      size_t n = strlen(hex) / 2;
      for (size_t i = 0; i < n; i++) {
        unsigned value;
        sscanf(hex + i * 2, "%2x", &value);
        bytes[i] = value;
      }
      color_t color = {0};
      if (rgb >= 0) {
        color.set = true;
        color.r = rgb >> 16; color.g = rgb >> 8; color.b = rgb;
      }
      screen_put_utf8(&screen, x, y, bytes, n, color, (color_t){0}, attrs);
    } else if (sscanf(line, "link %u %u", &x, &y) == 2) {
      const char *uri = "https://example.test";
      screen_set_link(&screen, x, y, screen_link_id(&screen, uri, strlen(uri)));
    } else if (sscanf(line, "cursor %u %u %u", &a, &x, &y) == 3) {
      screen.cursor_visible = a; screen.cursor_x = x; screen.cursor_y = y;
    } else if (sscanf(line, "hit %u %u %u %u", &x, &y, &a, &b) == 4) {
      hit_add(&screen.hits, x, y, a, b, "test:action");
    } else if (strcmp(line, "clear\n") == 0) {
      screen_clear(&screen);
    } else if (strcmp(line, "render\n") == 0) {
      screen_render(&screen);
      for (size_t i = 0; i < screen.out_len; i++) printf("%02x", (unsigned char)screen.out[i]);
      putchar('\n');
    } else if (strcmp(line, "snapshot\n") == 0) {
      char *json = screen_dump_json(&screen);
      puts(json); free(json);
    } else {
      fprintf(stderr, "unknown test command: %s", line); return 1;
    }
  }
  screen_free(&screen);
  return 0;
}
