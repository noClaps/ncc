/* Minimal host bridge: all lexing, parsing, name resolution, and C emission
 * live in compiler.nc. No compiler-runtime internals are required here. */
#include <stdio.h>
#include <stdlib.h>
const char *bootstrap_read_stdin(void) {
    size_t length = 0, capacity = 4096;
    char *text = malloc(capacity);
    if (!text) { fputs("out of memory\n", stderr); exit(1); }
    int c;
    while ((c = fgetc(stdin)) != EOF) {
        if (length + 1 == capacity) {
            if (capacity > (size_t)-1 / 2) { free(text); exit(1); }
            capacity *= 2;
            char *grown = realloc(text, capacity);
            if (!grown) { free(text); exit(1); }
            text = grown;
        }
        text[length++] = (char)c;
    }
    if (ferror(stdin)) { free(text); fputs("cannot read input\n", stderr); exit(1); }
    text[length] = 0;
    return text;
}
