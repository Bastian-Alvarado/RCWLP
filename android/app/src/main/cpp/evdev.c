// Raw Linux input (evdev) access for RCWLP's background mode.
//
// Runs inside EvdevService, which Shizuku starts with shell (adb) privileges: regular apps
// can't open /dev/input. Grabbing a controller makes its events go only to us, so the phone
// itself ignores it (no B-as-Back, no waking the screen) while it drives the PC.

#include <errno.h>
#include <fcntl.h>
#include <jni.h>
#include <linux/input.h>
#include <poll.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#define FN(name) Java_com_rcwlp_controller_Evdev_##name
#define MAX_EVENTS 64

JNIEXPORT jint JNICALL FN(open)(JNIEnv *env, jclass cls, jstring path) {
    const char *p = (*env)->GetStringUTFChars(env, path, NULL);
    if (!p) return -ENOMEM;
    int fd = open(p, O_RDONLY | O_CLOEXEC);
    int err = errno;
    (*env)->ReleaseStringUTFChars(env, path, p);
    return fd >= 0 ? fd : -err;
}

JNIEXPORT void JNICALL FN(close)(JNIEnv *env, jclass cls, jint fd) {
    close(fd);
}

JNIEXPORT jstring JNICALL FN(name)(JNIEnv *env, jclass cls, jint fd) {
    char buf[256] = {0};
    if (ioctl(fd, EVIOCGNAME(sizeof(buf) - 1), buf) < 0) return NULL;
    // NewStringUTF aborts on invalid modified UTF-8; device names are meant to be ASCII.
    for (char *c = buf; *c; c++) {
        if ((unsigned char)*c >= 0x80) *c = '?';
    }
    return (*env)->NewStringUTF(env, buf);
}

/** [bustype, vendor, product, version] */
JNIEXPORT jintArray JNICALL FN(id)(JNIEnv *env, jclass cls, jint fd) {
    struct input_id id;
    if (ioctl(fd, EVIOCGID, &id) < 0) return NULL;
    jint v[4] = {id.bustype, id.vendor, id.product, id.version};
    jintArray a = (*env)->NewIntArray(env, 4);
    if (a) (*env)->SetIntArrayRegion(env, a, 0, 4, v);
    return a;
}

static jbyteArray bits(JNIEnv *env, int fd, int type, int max) {
    unsigned char buf[KEY_MAX / 8 + 1];
    int len = max / 8 + 1;
    memset(buf, 0, sizeof(buf));
    if (ioctl(fd, EVIOCGBIT(type, len), buf) < 0) return NULL;
    jbyteArray a = (*env)->NewByteArray(env, len);
    if (a) (*env)->SetByteArrayRegion(env, a, 0, len, (const jbyte *)buf);
    return a;
}

JNIEXPORT jbyteArray JNICALL FN(keyBits)(JNIEnv *env, jclass cls, jint fd) {
    return bits(env, fd, EV_KEY, KEY_MAX);
}

JNIEXPORT jbyteArray JNICALL FN(absBits)(JNIEnv *env, jclass cls, jint fd) {
    return bits(env, fd, EV_ABS, ABS_MAX);
}

/** [value, min, max, fuzz, flat, resolution] */
JNIEXPORT jintArray JNICALL FN(absInfo)(JNIEnv *env, jclass cls, jint fd, jint axis) {
    struct input_absinfo ai;
    if (ioctl(fd, EVIOCGABS(axis), &ai) < 0) return NULL;
    jint v[6] = {ai.value, ai.minimum, ai.maximum, ai.fuzz, ai.flat, ai.resolution};
    jintArray a = (*env)->NewIntArray(env, 6);
    if (a) (*env)->SetIntArrayRegion(env, a, 0, 6, v);
    return a;
}

/** Returns 0 or -errno. */
JNIEXPORT jint JNICALL FN(grab)(JNIEnv *env, jclass cls, jint fd, jboolean on) {
    return ioctl(fd, EVIOCGRAB, (unsigned long)(on ? 1 : 0)) < 0 ? -errno : 0;
}

/**
 * Waits up to timeoutMs for events and stores them as [type, code, value] triples in out.
 * Returns the number of events, 0 on timeout, or -errno (-ENODEV once the device is gone).
 */
JNIEXPORT jint JNICALL FN(read)(JNIEnv *env, jclass cls, jint fd, jintArray out, jint timeoutMs) {
    struct pollfd p = {.fd = fd, .events = POLLIN};
    int r = poll(&p, 1, timeoutMs);
    if (r < 0) return errno == EINTR ? 0 : -errno;
    if (r == 0) return 0;
    if (p.revents & (POLLERR | POLLHUP | POLLNVAL)) return -ENODEV;

    int cap = (*env)->GetArrayLength(env, out) / 3;
    if (cap > MAX_EVENTS) cap = MAX_EVENTS;
    struct input_event ev[MAX_EVENTS];
    ssize_t n = read(fd, ev, sizeof(ev[0]) * cap);
    if (n < 0) return (errno == EAGAIN || errno == EINTR) ? 0 : -errno;

    int count = (int)(n / sizeof(ev[0]));
    jint triples[MAX_EVENTS * 3];
    for (int i = 0; i < count; i++) {
        triples[i * 3] = ev[i].type;
        triples[i * 3 + 1] = ev[i].code;
        triples[i * 3 + 2] = ev[i].value;
    }
    (*env)->SetIntArrayRegion(env, out, 0, count * 3, triples);
    return count;
}

static jstring string_ioctl(JNIEnv *env, int fd, unsigned long request) {
    char buf[256] = {0};
    if (ioctl(fd, request, buf) < 0) return NULL;
    for (char *c = buf; *c; c++) {
        if ((unsigned char)*c >= 0x80) *c = '?';
    }
    return (*env)->NewStringUTF(env, buf);
}

/** Where the device is attached, like "usb-xhci-hcd.0.auto-1/input0". Shared by a controller's interfaces. */
JNIEXPORT jstring JNICALL FN(phys)(JNIEnv *env, jclass cls, jint fd) {
    return string_ioctl(env, fd, EVIOCGPHYS(255));
}

/** A per-device id, usually the Bluetooth address; often empty for USB. */
JNIEXPORT jstring JNICALL FN(uniq)(JNIEnv *env, jclass cls, jint fd) {
    return string_ioctl(env, fd, EVIOCGUNIQ(255));
}
