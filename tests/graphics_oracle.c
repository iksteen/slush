/* Test-only oracle: the unchanged C wire emitter with deterministic frame inputs. */
#include "graphics.h"
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
  graphics_t *g = gfx_new();
  char line[4096];
  while (fgets(line, sizeof line, stdin)) {
    if (!strcmp(line, "begin\n"))
      gfx_begin(g);
    else if (!strcmp(line, "reset\n"))
      gfx_reset(g);
    else if (!strcmp(line, "repaint\n"))
      gfx_repaint(g);
    else if (!strncmp(line, "commit ", 7))
      gfx_commit(g, atoi(line + 7));
    else if (!strncmp(line, "forget ", 7))
      gfx_forget_pane(g, atoi(line + 7));
    else if (!strncmp(line, "place ", 6)) {
      gfx_req_t r = {0};
      unsigned piece, x, y, cols, rows, sc, sr, seed, count;
      int got = sscanf(
          line + 6,
          "%u %u %" SCNu64
          " %u %u %u %u %u %u %u %u %u %u %u %u %u %u %u %u %d %d %u %u",
          &r.pane, &r.src_id, &r.gen, &r.place_id, &piece, &x, &y, &cols, &rows,
          &sc, &sr, &r.x_off, &r.y_off, &r.sx, &r.sy, &r.sw, &r.sh, &r.px_w,
          &r.px_h, &r.format, &r.compression, &seed, &count);
      if (got != 23 || count > 8192) return 2;
      r.piece = piece;
      r.col = x;
      r.row = y;
      r.cols = cols;
      r.rows = rows;
      r.scale_cols = sc;
      r.scale_rows = sr;
      uint8_t data[8192];
      for (unsigned i = 0; i < count; i++) data[i] = (seed + i * 17) & 255;
      r.data = data;
      r.data_len = count;
      gfx_place(g, &r);
    } else if (!strcmp(line, "flush\n")) {
      size_t len;
      const char *bytes = gfx_flush(g, &len);
      for (size_t i = 0; i < len; i++) printf("%02x", (unsigned char)bytes[i]);
      putchar('\n');
      const gfx_place_t *p;
      size_t count = gfx_placements(g, &p);
      for (size_t i = 0; i < count; i++)
        printf("%u,%u,%u,%u,%u,%u,%u,%u,%u;", p[i].out_id, p[i].place_id,
               p[i].piece, p[i].col, p[i].row, p[i].cols, p[i].rows, p[i].x_off,
               p[i].y_off);
      putchar('\n');
    } else
      return 3;
  }
  gfx_free(g);
  return 0;
}
