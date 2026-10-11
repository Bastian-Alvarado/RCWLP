package com.rcwlp.controller;

import com.rcwlp.controller.IEvdevCallback;

/** Implemented by EvdevService, which Shizuku runs with shell privileges. */
interface IEvdevService {
    /** Reserved by Shizuku: called when the service is unbound with remove=true. */
    void destroy() = 16777114;

    /** Finds attached controllers (and keeps watching for new ones), grabs them and reports their state. */
    void start(IEvdevCallback callback) = 1;

    /**
     * Xbox mode: sends INPUT for this session from this process from now on, starting after
     * firstSeq, and reports each STATUS through IEvdevCallback.onStatus. Dead zones are in
     * percent. Returns false if it couldn't start; the app then keeps sending itself.
     */
    boolean startStreaming(in byte[] sessionKey, in byte[] clientId, String hostIp, int hostPort, int firstSeq,
        int stickDeadzone, int triggerDeadzone) = 2;

    /** Stops streaming. Returns the last seq sent, so DISCONNECT can follow it, or -1 if it wasn't streaming. */
    int stopStreaming() = 3;

    /** The phone's battery, in INPUT's encoding, for the packets this process sends. */
    oneway void setBattery(int battery) = 4;

    /** Milliseconds since this process last got a STATUS while streaming, or -1 if it isn't streaming. */
    long msSinceStatus() = 5;
}
