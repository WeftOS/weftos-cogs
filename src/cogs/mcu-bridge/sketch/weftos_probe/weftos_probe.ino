// WeftOS probe sketch: proves which firmware is on the MCU and lets mcu-bridge check it.
// Flash it with the operator guide (guide/sketch.md); it replaces whatever App Lab app is on the MCU.
#include <Arduino_RouterBridge.h>

static unsigned long calls = 0;

int weftos_ping() { return 42; }
unsigned long weftos_calls() { return ++calls; }

void setup() {
    Bridge.begin();
    Bridge.provide("weftos_ping", weftos_ping);
    Bridge.provide("weftos_calls", weftos_calls);
}

void loop() {}
