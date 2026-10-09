package com.rcwlp.controller;

/** EvdevService reports back to the app through this. */
oneway interface IEvdevCallback {
    /** Merged state of all grabbed controllers, in XInput terms like PadState. */
    void onState(int buttons, int leftTrigger, int rightTrigger, int lx, int ly, int rx, int ry);

    /** Names of the controllers currently being read, one per line; empty when none. */
    void onDevices(String names);
}
