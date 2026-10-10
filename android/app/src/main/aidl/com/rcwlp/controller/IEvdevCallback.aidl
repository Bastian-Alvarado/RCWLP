package com.rcwlp.controller;

/** EvdevService reports back to the app through this. */
oneway interface IEvdevCallback {
    /** One controller's state, in XInput terms like PadState. Each physical controller keeps its slot. */
    void onState(int slot, int buttons, int leftTrigger, int rightTrigger, int lx, int ly, int rx, int ry);

    /** The controller in this slot was detached. */
    void onRemoved(int slot);

    /** Names of the controllers currently being read, one per line as "slot:name"; empty when none. */
    void onDevices(String names);
}
