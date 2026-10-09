package com.rcwlp.controller;

import com.rcwlp.controller.IEvdevCallback;

/** Implemented by EvdevService, which Shizuku runs with shell privileges. */
interface IEvdevService {
    /** Reserved by Shizuku: called when the service is unbound with remove=true. */
    void destroy() = 16777114;

    /** Finds attached controllers (and keeps watching for new ones), grabs them and reports their state. */
    void start(IEvdevCallback callback) = 1;
}
