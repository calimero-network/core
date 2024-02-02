#include <emscripten.h>

EMSCRIPTEN_KEEPALIVE
int counter = 0;

EMSCRIPTEN_KEEPALIVE
void increment() {
    counter++;
}

EMSCRIPTEN_KEEPALIVE
int get_counter() {
    return counter;
}
