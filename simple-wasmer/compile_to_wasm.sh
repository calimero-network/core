emcc increment.c -s EXPORTED_FUNCTIONS='["_increment","_get_counter"]' -s ALLOW_MEMORY_GROWTH=1 -o module.js
