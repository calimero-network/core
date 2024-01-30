#include <stdint.h>

uint32_t counter = 0;

void increment() {
    counter++;
}

uint32_t get_counter() {
    return counter;
}
