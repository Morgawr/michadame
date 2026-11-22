#define _DEFAULT_SOURCE
#define _POSIX_C_SOURCE 200809L

#include "chiaki_bridge.h"

#include <chiaki/common.h>
#include <chiaki/controller.h>
#include <chiaki/ctrl.h>
#include <chiaki/discovery.h>
#include <chiaki/discoveryservice.h>
#include <chiaki/log.h>
#include <chiaki/opusdecoder.h>
#include <chiaki/orientation.h>
#include <chiaki/regist.h>
#include <chiaki/remote/holepunch.h>
#include <chiaki/session.h>
#include <chiaki/time.h>

#include <SDL.h>

#include <arpa/inet.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>

#define PS5_SONY_VENDOR_ID 0x054c
#define PS5_DUALSENSE_PRODUCT_ID 0x0ce6
#define PS5_DUALSENSE_EDGE_PRODUCT_ID 0x0df2

#define PS5_TOUCHPAD_MAX_X 1919.0f
#define PS5_TOUCHPAD_MAX_Y 1079.0f

static pthread_once_t g_chiaki_init_once = PTHREAD_ONCE_INIT;
static int g_chiaki_init_result = -1;

static void do_chiaki_lib_init(void) {
    ChiakiErrorCode err = chiaki_lib_init();
    g_chiaki_init_result = (err == CHIAKI_ERR_SUCCESS) ? 0 : (int)err;
}

int michadame_ps5_lib_init(void) {
    pthread_once(&g_chiaki_init_once, do_chiaki_lib_init);
    return g_chiaki_init_result;
}

static void write_err(char *buf, size_t cap, const char *msg) {
    if (!buf || cap == 0) {
        return;
    }
    if (!msg) {
        buf[0] = '\0';
        return;
    }
    snprintf(buf, cap, "%s", msg);
}

static void copy_str_safe(char *dst, size_t cap, const char *src) {
    if (!dst || cap == 0) {
        return;
    }
    if (!src) {
        dst[0] = '\0';
        return;
    }
    snprintf(dst, cap, "%s", src);
}

static bool hex_to_bytes(const char *hex, uint8_t *out, size_t out_len) {
    if (!hex || strlen(hex) != out_len * 2) {
        return false;
    }
    for (size_t i = 0; i < out_len; i++) {
        char hi = hex[i * 2];
        char lo = hex[i * 2 + 1];
        uint8_t v_hi, v_lo;
        if (hi >= '0' && hi <= '9') v_hi = (uint8_t)(hi - '0');
        else if (hi >= 'a' && hi <= 'f') v_hi = (uint8_t)(hi - 'a' + 10);
        else if (hi >= 'A' && hi <= 'F') v_hi = (uint8_t)(hi - 'A' + 10);
        else return false;

        if (lo >= '0' && lo <= '9') v_lo = (uint8_t)(lo - '0');
        else if (lo >= 'a' && lo <= 'f') v_lo = (uint8_t)(lo - 'a' + 10);
        else if (lo >= 'A' && lo <= 'F') v_lo = (uint8_t)(lo - 'A' + 10);
        else return false;

        out[i] = (uint8_t)((v_hi << 4) | v_lo);
    }
    return true;
}

int michadame_ps5_generate_client_duid(char *out_buf, size_t out_cap) {
    if (!out_buf || out_cap < CHIAKI_DUID_STR_SIZE) {
        return -1;
    }
    if (michadame_ps5_lib_init() != 0) {
        return -1;
    }
    size_t sz = out_cap;
    ChiakiErrorCode err = chiaki_holepunch_generate_client_device_uid(out_buf, &sz);
    if (err != CHIAKI_ERR_SUCCESS) {
        return (int)err;
    }
    if (sz >= out_cap) {
        out_buf[out_cap - 1] = '\0';
    } else {
        out_buf[sz] = '\0';
    }
    return 0;
}

/* ===========================
 * Discovery & Wakeup
 * =========================== */

static void silent_log_cb(ChiakiLogLevel level, const char *msg, void *user) {
    (void)level;
    (void)msg;
    (void)user;
}

typedef struct {
    pthread_mutex_t mu;
    MichadamePs5DiscoveredHost hosts[32];
    size_t count;
} DiscoveryCollector;

static void discovery_collector_cb(ChiakiDiscoveryHost *hosts, size_t hosts_count, void *user) {
    DiscoveryCollector *col = (DiscoveryCollector *)user;
    pthread_mutex_lock(&col->mu);
    for (size_t i = 0; i < hosts_count; i++) {
        ChiakiDiscoveryHost *h = &hosts[i];
        bool is_ps5 = chiaki_discovery_host_is_ps5(h);
        if (!is_ps5) {
            continue;
        }
        const char *addr = h->host_addr ? h->host_addr : "";
        const char *id = h->host_id ? h->host_id : "";

        /* Deduplicate by host_id (MAC) or host_addr */
        size_t slot = col->count;
        for (size_t j = 0; j < col->count; j++) {
            if ((id[0] != '\0' && strcmp(col->hosts[j].host_id, id) == 0) ||
                (addr[0] != '\0' && strcmp(col->hosts[j].host_addr, addr) == 0)) {
                slot = j;
                break;
            }
        }
        if (slot >= 32) {
            continue;
        }
        if (slot == col->count) {
            col->count++;
        }
        MichadamePs5DiscoveredHost *dst = &col->hosts[slot];
        memset(dst, 0, sizeof(*dst));
        dst->state = (int)h->state;
        dst->host_request_port = h->host_request_port;
        dst->is_ps5 = true;
        copy_str_safe(dst->host_addr, sizeof(dst->host_addr), h->host_addr);
        copy_str_safe(dst->host_name, sizeof(dst->host_name), h->host_name);
        copy_str_safe(dst->host_id, sizeof(dst->host_id), h->host_id);
        copy_str_safe(dst->host_type, sizeof(dst->host_type), h->host_type);
        copy_str_safe(dst->system_version, sizeof(dst->system_version), h->system_version);
        copy_str_safe(dst->running_app_titleid, sizeof(dst->running_app_titleid), h->running_app_titleid);
        copy_str_safe(dst->running_app_name, sizeof(dst->running_app_name), h->running_app_name);
    }
    pthread_mutex_unlock(&col->mu);
}

int michadame_ps5_discover_once(
    uint32_t timeout_ms,
    MichadamePs5DiscoveredHostCb cb,
    void *user
) {
    if (michadame_ps5_lib_init() != 0) {
        return -1;
    }

    ChiakiLog log;
    chiaki_log_init(&log, CHIAKI_LOG_ERROR | CHIAKI_LOG_WARNING, silent_log_cb, NULL);

    DiscoveryCollector col;
    memset(&col, 0, sizeof(col));
    pthread_mutex_init(&col.mu, NULL);

    /* Collect all active IPv4 broadcast addresses across interfaces */
    struct sockaddr_storage bcast_addrs[32];
    size_t bcast_count = 0;

    struct ifaddrs *ifaddr = NULL;
    if (getifaddrs(&ifaddr) == 0) {
        for (struct ifaddrs *ifa = ifaddr; ifa != NULL && bcast_count < 31; ifa = ifa->ifa_next) {
            if (!ifa->ifa_addr || !ifa->ifa_broadaddr) {
                continue;
            }
            if ((ifa->ifa_flags & (IFF_UP | IFF_RUNNING | IFF_LOOPBACK | IFF_BROADCAST)) !=
                (IFF_UP | IFF_RUNNING | IFF_BROADCAST)) {
                continue;
            }
            if (ifa->ifa_addr->sa_family != AF_INET) {
                continue;
            }
            struct sockaddr_in *b = (struct sockaddr_in *)ifa->ifa_broadaddr;
            bool dup = false;
            for (size_t k = 0; k < bcast_count; k++) {
                struct sockaddr_in *prev = (struct sockaddr_in *)&bcast_addrs[k];
                if (prev->sin_addr.s_addr == b->sin_addr.s_addr) {
                    dup = true;
                    break;
                }
            }
            if (!dup) {
                memset(&bcast_addrs[bcast_count], 0, sizeof(struct sockaddr_storage));
                struct sockaddr_in *dst = (struct sockaddr_in *)&bcast_addrs[bcast_count];
                dst->sin_family = AF_INET;
                dst->sin_addr = b->sin_addr;
                bcast_count++;
            }
        }
        freeifaddrs(ifaddr);
    }

    struct sockaddr_storage fallback_addr;
    memset(&fallback_addr, 0, sizeof(fallback_addr));
    struct sockaddr_in *in_fallback = (struct sockaddr_in *)&fallback_addr;
    in_fallback->sin_family = AF_INET;
    in_fallback->sin_addr.s_addr = 0xffffffffu;

    ChiakiDiscoveryServiceOptions opts;
    memset(&opts, 0, sizeof(opts));
    opts.hosts_max = 16;
    opts.host_drop_pings = 3;
    opts.ping_ms = 400;
    opts.ping_initial_ms = 100;
    opts.send_addr = &fallback_addr;
    opts.send_addr_size = sizeof(struct sockaddr_in);
    if (bcast_count > 0) {
        opts.broadcast_addrs = bcast_addrs;
        opts.broadcast_num = bcast_count;
    }
    opts.cb = discovery_collector_cb;
    opts.cb_user = &col;

    ChiakiDiscoveryService svc;
    ChiakiErrorCode err = chiaki_discovery_service_init(&svc, &opts, &log);
    if (err != CHIAKI_ERR_SUCCESS) {
        pthread_mutex_destroy(&col.mu);
        return (int)err;
    }

    if (timeout_ms < 500) {
        timeout_ms = 500;
    }
    struct timespec ts;
    ts.tv_sec = (time_t)(timeout_ms / 1000);
    ts.tv_nsec = (long)(timeout_ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);

    chiaki_discovery_service_fini(&svc);

    if (cb) {
        pthread_mutex_lock(&col.mu);
        for (size_t i = 0; i < col.count; i++) {
            cb(&col.hosts[i], user);
        }
        pthread_mutex_unlock(&col.mu);
    }

    pthread_mutex_destroy(&col.mu);
    return 0;
}

int michadame_ps5_wakeup(const char *host, const uint8_t regist_key[16]) {
    if (!host || !regist_key || host[0] == '\0') {
        return -1;
    }
    if (michadame_ps5_lib_init() != 0) {
        return -1;
    }
    char key_str[17];
    memcpy(key_str, regist_key, 16);
    key_str[16] = '\0';
    char *endptr = NULL;
    uint64_t cred = strtoull(key_str, &endptr, 16);
    if (endptr == key_str || cred == 0) {
        return -1;
    }
    ChiakiLog log;
    chiaki_log_init(&log, CHIAKI_LOG_ERROR | CHIAKI_LOG_WARNING, silent_log_cb, NULL);
    ChiakiErrorCode err = chiaki_discovery_wakeup(&log, NULL, host, cred, true);
    return (err == CHIAKI_ERR_SUCCESS) ? 0 : (int)err;
}

/* ===========================
 * PSN Remote Device Listing
 * =========================== */

int michadame_ps5_psn_list_devices(
    const char *psn_oauth_token,
    MichadamePs5PsnDeviceCb cb,
    void *user,
    char *err_buf,
    size_t err_cap
) {
    if (!psn_oauth_token || psn_oauth_token[0] == '\0') {
        write_err(err_buf, err_cap, "Missing PSN OAuth token");
        return -1;
    }
    if (michadame_ps5_lib_init() != 0) {
        write_err(err_buf, err_cap, "Failed to initialize libchiaki");
        return -1;
    }
    ChiakiLog log;
    chiaki_log_init(&log, CHIAKI_LOG_ERROR | CHIAKI_LOG_WARNING, silent_log_cb, NULL);

    ChiakiHolepunchDeviceInfo *devices = NULL;
    size_t count = 0;
    ChiakiErrorCode err = chiaki_holepunch_list_devices(
        psn_oauth_token,
        CHIAKI_HOLEPUNCH_CONSOLE_TYPE_PS5,
        &devices,
        &count,
        &log
    );
    if (err != CHIAKI_ERR_SUCCESS) {
        char msg[160];
        snprintf(msg, sizeof(msg), "PSN device query failed: %s", chiaki_error_string(err));
        write_err(err_buf, err_cap, msg);
        return (int)err;
    }

    if (cb && devices) {
        for (size_t i = 0; i < count; i++) {
            MichadamePs5PsnDevice out;
            memset(&out, 0, sizeof(out));
            copy_str_safe(out.device_name, sizeof(out.device_name), devices[i].device_name);
            for (size_t b = 0; b < sizeof(devices[i].device_uid); b++) {
                snprintf(&out.device_uid_hex[b * 2], 3, "%02x", devices[i].device_uid[b]);
            }
            out.device_uid_hex[64] = '\0';
            out.remoteplay_enabled = devices[i].remoteplay_enabled;
            out.is_ps5 = (devices[i].type == CHIAKI_HOLEPUNCH_CONSOLE_TYPE_PS5);
            cb(&out, user);
        }
    }

    if (devices) {
        chiaki_holepunch_free_device_list(&devices);
    }
    return 0;
}

/* ===========================
 * Local PIN Registration
 * =========================== */

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t cond;
    bool done;
    bool success;
    MichadamePs5RegistResult result;
    char last_error[256];
} LocalRegistWaiter;

static void local_regist_log_cb(ChiakiLogLevel level, const char *msg, void *user) {
    LocalRegistWaiter *w = (LocalRegistWaiter *)user;
    if (level == CHIAKI_LOG_ERROR && msg) {
        pthread_mutex_lock(&w->mu);
        copy_str_safe(w->last_error, sizeof(w->last_error), msg);
        pthread_mutex_unlock(&w->mu);
    }
}

static void fill_regist_result(MichadamePs5RegistResult *dst, const ChiakiRegisteredHost *src) {
    memset(dst, 0, sizeof(*dst));
    if (!src) {
        return;
    }
    memcpy(dst->server_mac, src->server_mac, 6);
    copy_str_safe(dst->server_nickname, sizeof(dst->server_nickname), src->server_nickname);
    memcpy(dst->rp_regist_key, src->rp_regist_key, 16);
    memcpy(dst->rp_key, src->rp_key, 16);
    dst->rp_key_type = src->rp_key_type;
    dst->console_pin = src->console_pin;
    dst->target = (int)src->target;
}

static void local_regist_cb(ChiakiRegistEvent *event, void *user) {
    LocalRegistWaiter *w = (LocalRegistWaiter *)user;
    pthread_mutex_lock(&w->mu);
    if (event->type == CHIAKI_REGIST_EVENT_TYPE_FINISHED_SUCCESS && event->registered_host) {
        fill_regist_result(&w->result, event->registered_host);
        w->success = true;
    } else {
        w->success = false;
    }
    w->done = true;
    pthread_cond_signal(&w->cond);
    pthread_mutex_unlock(&w->mu);
}

int michadame_ps5_register_local(
    const char *host,
    bool broadcast,
    const uint8_t psn_account_id[8],
    uint32_t pin,
    uint32_t console_pin,
    MichadamePs5RegistResult *out_result,
    char *err_buf,
    size_t err_cap
) {
    if (!host || host[0] == '\0' || !psn_account_id || !out_result) {
        write_err(err_buf, err_cap, "Invalid registration arguments");
        return -1;
    }
    if (michadame_ps5_lib_init() != 0) {
        write_err(err_buf, err_cap, "Failed to initialize libchiaki");
        return -1;
    }

    LocalRegistWaiter waiter;
    memset(&waiter, 0, sizeof(waiter));
    pthread_mutex_init(&waiter.mu, NULL);
    pthread_cond_init(&waiter.cond, NULL);

    ChiakiLog log;
    chiaki_log_init(&log, CHIAKI_LOG_ALL & ~CHIAKI_LOG_VERBOSE, local_regist_log_cb, &waiter);

    ChiakiRegistInfo info;
    memset(&info, 0, sizeof(info));
    info.target = CHIAKI_TARGET_PS5_1;
    info.host = host;
    info.broadcast = broadcast;
    info.psn_online_id = NULL;
    memcpy(info.psn_account_id, psn_account_id, CHIAKI_PSN_ACCOUNT_ID_SIZE);
    info.pin = pin;
    info.console_pin = console_pin;
    info.holepunch_info = NULL;
    info.rudp = NULL;

    ChiakiRegist regist;
    memset(&regist, 0, sizeof(regist));
    ChiakiErrorCode err = chiaki_regist_start(&regist, &log, &info, local_regist_cb, &waiter);
    if (err != CHIAKI_ERR_SUCCESS) {
        char msg[160];
        snprintf(msg, sizeof(msg), "Registration failed to start: %s", chiaki_error_string(err));
        write_err(err_buf, err_cap, msg);
        pthread_cond_destroy(&waiter.cond);
        pthread_mutex_destroy(&waiter.mu);
        return (int)err;
    }

    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_sec += 20;

    pthread_mutex_lock(&waiter.mu);
    while (!waiter.done) {
        if (pthread_cond_timedwait(&waiter.cond, &waiter.mu, &deadline) != 0) {
            break;
        }
    }
    bool ok = waiter.done && waiter.success;
    if (ok) {
        *out_result = waiter.result;
    } else if (!waiter.done) {
        write_err(err_buf, err_cap, "Registration timed out (check PS5 IP, PIN, and PSN Account ID)");
    } else if (waiter.last_error[0] != '\0') {
        write_err(err_buf, err_cap, waiter.last_error);
    } else {
        write_err(err_buf, err_cap, "Registration failed (verify 8-digit PIN and PSN Account ID)");
    }
    pthread_mutex_unlock(&waiter.mu);

    chiaki_regist_stop(&regist);
    chiaki_regist_fini(&regist);

    pthread_cond_destroy(&waiter.cond);
    pthread_mutex_destroy(&waiter.mu);
    return ok ? 0 : -1;
}

/* ===========================
 * Streaming Session
 * =========================== */

struct MichadamePs5Session {
    ChiakiSession session;
    ChiakiOpusDecoder opus_decoder;
    ChiakiLog log;
    ChiakiHolepunchSession holepunch_session; /* owned by session after chiaki_session_init */

    MichadamePs5VideoCb video_cb;
    MichadamePs5AudioSettingsCb audio_settings_cb;
    MichadamePs5AudioFrameCb audio_frame_cb;
    MichadamePs5HapticsFrameCb haptics_frame_cb;
    MichadamePs5EventCb event_cb;
    MichadamePs5LogCb log_cb;
    void *user;

    pthread_mutex_t lifecycle_mu;
    bool session_initialized;
    bool session_started;
    bool stop_requested;
    bool holepunch_in_flight;
    bool enable_keyboard;
};

static void bridge_session_log_cb(ChiakiLogLevel level, const char *msg, void *user) {
    MichadamePs5Session *h = (MichadamePs5Session *)user;
    if (h && h->log_cb && msg) {
        h->log_cb((int)level, msg, h->user);
    }
}

static bool bridge_video_sample_cb(
    uint8_t *buf,
    size_t buf_size,
    int32_t frames_lost,
    bool frame_recovered,
    void *user
) {
    MichadamePs5Session *h = (MichadamePs5Session *)user;
    if (!h || !h->video_cb) {
        return true;
    }
    return h->video_cb(buf, buf_size, frames_lost, frame_recovered, h->user);
}

static void bridge_audio_settings_cb(uint32_t channels, uint32_t rate, void *user) {
    MichadamePs5Session *h = (MichadamePs5Session *)user;
    if (h && h->audio_settings_cb) {
        h->audio_settings_cb(channels, rate, h->user);
    }
}

static void bridge_audio_frame_cb(int16_t *buf, size_t samples_count, void *user) {
    MichadamePs5Session *h = (MichadamePs5Session *)user;
    if (h && h->audio_frame_cb) {
        h->audio_frame_cb(buf, samples_count, h->user);
    }
}

static void bridge_haptics_frame_cb(uint8_t *buf, size_t buf_size, void *user) {
    MichadamePs5Session *h = (MichadamePs5Session *)user;
    if (h && h->haptics_frame_cb) {
        h->haptics_frame_cb(buf, buf_size, h->user);
    }
}

static void bridge_cant_display_cb(void *user, bool cant_display) {
    MichadamePs5Session *h = (MichadamePs5Session *)user;
    if (!h || !h->event_cb) {
        return;
    }
    MichadamePs5Event ev;
    memset(&ev, 0, sizeof(ev));
    ev.type = MICHADAME_PS5_EV_CANT_DISPLAY;
    ev.flag = cant_display;
    h->event_cb(&ev, h->user);
}

static void bridge_event_cb(ChiakiEvent *event, void *user) {
    MichadamePs5Session *h = (MichadamePs5Session *)user;
    if (!h || !h->event_cb || !event) {
        return;
    }

    MichadamePs5Event ev;
    memset(&ev, 0, sizeof(ev));

    switch (event->type) {
        case CHIAKI_EVENT_CONNECTED:
            if (h->enable_keyboard) {
                /* CTRL_MESSAGE_TYPE_KEYBOARD_ENABLE_TOGGLE (0x20): enable IME keyboard
                 * prompts only after the stream is connected, avoiding 0x000D during
                 * handshake where it collides with PSN RUDP switch-to-stream. */
                const uint8_t enable = 1;
                chiaki_ctrl_send_message(&h->session.ctrl, 0x20, &enable, 1);
            }
            ev.type = MICHADAME_PS5_EV_CONNECTED;
            break;
        case CHIAKI_EVENT_QUIT: {
            ev.type = MICHADAME_PS5_EV_QUIT;
            ev.quit_reason = (int)event->quit.reason;
            ev.quit_is_error = chiaki_quit_reason_is_error(event->quit.reason);
            const char *base = chiaki_quit_reason_string(event->quit.reason);
            char combined[256];
            if (event->quit.reason_str && event->quit.reason_str[0] != '\0') {
                snprintf(combined, sizeof(combined), "%s (%s)", base, event->quit.reason_str);
                ev.text = combined;
            } else {
                ev.text = base;
            }
            h->event_cb(&ev, h->user);
            return;
        }
        case CHIAKI_EVENT_LOGIN_PIN_REQUEST:
            ev.type = MICHADAME_PS5_EV_LOGIN_PIN_REQUEST;
            ev.flag = event->login_pin_request.pin_incorrect;
            break;
        case CHIAKI_EVENT_REGIST:
            ev.type = MICHADAME_PS5_EV_REGIST;
            fill_regist_result(&ev.regist, &event->host);
            break;
        case CHIAKI_EVENT_NICKNAME_RECEIVED:
            ev.type = MICHADAME_PS5_EV_NICKNAME;
            ev.text = event->server_nickname;
            break;
        case CHIAKI_EVENT_KEYBOARD_OPEN:
            ev.type = MICHADAME_PS5_EV_KEYBOARD_OPEN;
            ev.text = event->keyboard.text_str ? event->keyboard.text_str : "";
            break;
        case CHIAKI_EVENT_KEYBOARD_TEXT_CHANGE:
            ev.type = MICHADAME_PS5_EV_KEYBOARD_TEXT_CHANGE;
            ev.text = event->keyboard.text_str ? event->keyboard.text_str : "";
            break;
        case CHIAKI_EVENT_KEYBOARD_REMOTE_CLOSE:
            ev.type = MICHADAME_PS5_EV_KEYBOARD_REMOTE_CLOSE;
            break;
        case CHIAKI_EVENT_RUMBLE:
            ev.type = MICHADAME_PS5_EV_RUMBLE;
            ev.rumble_left = event->rumble.left;
            ev.rumble_right = event->rumble.right;
            break;
        case CHIAKI_EVENT_TRIGGER_EFFECTS:
            ev.type = MICHADAME_PS5_EV_TRIGGER_EFFECTS;
            ev.trigger_type_left = event->trigger_effects.type_left;
            ev.trigger_type_right = event->trigger_effects.type_right;
            memcpy(ev.trigger_left, event->trigger_effects.left, 10);
            memcpy(ev.trigger_right, event->trigger_effects.right, 10);
            break;
        case CHIAKI_EVENT_LED_COLOR:
            ev.type = MICHADAME_PS5_EV_LED_COLOR;
            memcpy(ev.led_color, event->led_state, 3);
            break;
        case CHIAKI_EVENT_PLAYER_INDEX:
            ev.type = MICHADAME_PS5_EV_PLAYER_INDEX;
            ev.player_index = event->player_index;
            break;
        case CHIAKI_EVENT_HAPTIC_INTENSITY:
            ev.type = MICHADAME_PS5_EV_HAPTIC_INTENSITY;
            ev.intensity = (int)event->intensity;
            break;
        case CHIAKI_EVENT_TRIGGER_INTENSITY:
            ev.type = MICHADAME_PS5_EV_TRIGGER_INTENSITY;
            ev.intensity = (int)event->intensity;
            break;
        case CHIAKI_EVENT_MOTION_RESET:
            ev.type = MICHADAME_PS5_EV_MOTION_RESET;
            break;
        case CHIAKI_EVENT_HOLEPUNCH:
            ev.type = MICHADAME_PS5_EV_HOLEPUNCH;
            ev.flag = event->data_holepunch.finished;
            break;
        case CHIAKI_EVENT_VIDEO_FEC_FAILURE:
            ev.type = MICHADAME_PS5_EV_FEC_FAILURE;
            ev.frame_index = event->video_fec_failure.frame_index;
            ev.flag = event->video_fec_failure.idr_request_sent;
            break;
        default:
            return;
    }

    h->event_cb(&ev, h->user);
}

MichadamePs5Session *michadame_ps5_session_create(
    const MichadamePs5SessionConfig *cfg,
    char *err_buf,
    size_t err_cap
) {
    if (!cfg) {
        write_err(err_buf, err_cap, "Null PS5 session config");
        return NULL;
    }
    if (michadame_ps5_lib_init() != 0) {
        write_err(err_buf, err_cap, "Failed to initialize libchiaki");
        return NULL;
    }

    MichadamePs5Session *h = (MichadamePs5Session *)calloc(1, sizeof(MichadamePs5Session));
    if (!h) {
        write_err(err_buf, err_cap, "Out of memory allocating PS5 session");
        return NULL;
    }

    pthread_mutex_init(&h->lifecycle_mu, NULL);
    h->video_cb = cfg->video_cb;
    h->audio_settings_cb = cfg->audio_settings_cb;
    h->audio_frame_cb = cfg->audio_frame_cb;
    h->haptics_frame_cb = cfg->haptics_frame_cb;
    h->event_cb = cfg->event_cb;
    h->log_cb = cfg->log_cb;
    h->user = cfg->user;
    h->enable_keyboard = cfg->enable_keyboard;

    chiaki_log_init(&h->log, CHIAKI_LOG_ALL & ~CHIAKI_LOG_VERBOSE, bridge_session_log_cb, h);

    ChiakiConnectInfo ci;
    memset(&ci, 0, sizeof(ci));
    ci.ps5 = true;
    ci.host = (cfg->host && cfg->host[0] != '\0') ? cfg->host : "";
    ci.video_profile.width = cfg->width ? cfg->width : 1920;
    ci.video_profile.height = cfg->height ? cfg->height : 1080;
    ci.video_profile.max_fps = cfg->max_fps ? cfg->max_fps : 60;
    ci.video_profile.bitrate = cfg->bitrate_kbps ? cfg->bitrate_kbps : 15000;
    ci.video_profile.codec = (cfg->codec == 0) ? CHIAKI_CODEC_H264 : CHIAKI_CODEC_H265;
    ci.video_profile_auto_downgrade = true;
    /* Match chiaki-ng (streamsession.cpp): keep enable_keyboard=false during handshake.
     * Enabling it in libchiaki causes ctrl_enable_features() to send Ctrl message 0x000D
     * (16 bytes) before Senkusha, which collides with PSN RUDP's switch-to-stream packet
     * (also 0x000D, 16 bytes), breaking Senkusha and causing PS5 to shut down the stream. */
    ci.enable_keyboard = false;
    ci.enable_dualsense = cfg->enable_dualsense;
    ci.audio_video_disabled = CHIAKI_NONE_DISABLED;
    ci.auto_regist = cfg->auto_regist;
    ci.packet_loss_max = 0.05;
    ci.enable_idr_on_fec_failure = true;

    bool use_psn = cfg->psn_oauth_token && cfg->psn_oauth_token[0] != '\0' &&
                   cfg->psn_duid_hex && cfg->psn_duid_hex[0] != '\0';

    if (use_psn) {
        h->holepunch_session = chiaki_holepunch_session_init(cfg->psn_oauth_token, &h->log);
        if (!h->holepunch_session) {
            write_err(err_buf, err_cap, "Failed to initialize PSN holepunch session");
            pthread_mutex_destroy(&h->lifecycle_mu);
            free(h);
            return NULL;
        }
        ci.holepunch_session = h->holepunch_session;
        memcpy(ci.psn_account_id, cfg->psn_account_id, CHIAKI_PSN_ACCOUNT_ID_SIZE);
    } else {
        memcpy(ci.regist_key, cfg->regist_key, sizeof(ci.regist_key));
        memcpy(ci.morning, cfg->morning, sizeof(ci.morning));
    }

    ChiakiErrorCode err = chiaki_session_init(&h->session, &ci, &h->log);
    if (err != CHIAKI_ERR_SUCCESS) {
        /* Note: chiaki_session_init frees holepunch_session on error */
        h->holepunch_session = NULL;
        char msg[160];
        snprintf(msg, sizeof(msg), "Chiaki session init failed: %s", chiaki_error_string(err));
        write_err(err_buf, err_cap, msg);
        pthread_mutex_destroy(&h->lifecycle_mu);
        free(h);
        return NULL;
    }

    h->session_initialized = true;

    ChiakiCtrlDisplaySink display_sink;
    memset(&display_sink, 0, sizeof(display_sink));
    display_sink.user = h;
    display_sink.cantdisplay_cb = bridge_cant_display_cb;
    chiaki_session_ctrl_set_display_sink(&h->session, &display_sink);

    chiaki_opus_decoder_init(&h->opus_decoder, &h->log);
    chiaki_opus_decoder_set_cb(
        &h->opus_decoder,
        bridge_audio_settings_cb,
        bridge_audio_frame_cb,
        h
    );
    ChiakiAudioSink audio_sink;
    memset(&audio_sink, 0, sizeof(audio_sink));
    chiaki_opus_decoder_get_sink(&h->opus_decoder, &audio_sink);
    chiaki_session_set_audio_sink(&h->session, &audio_sink);

    if (cfg->enable_dualsense && cfg->haptics_frame_cb) {
        ChiakiAudioSink haptics_sink;
        memset(&haptics_sink, 0, sizeof(haptics_sink));
        haptics_sink.user = h;
        haptics_sink.frame_cb = bridge_haptics_frame_cb;
        chiaki_session_set_haptics_sink(&h->session, &haptics_sink);
    }

    chiaki_session_set_video_sample_cb(&h->session, bridge_video_sample_cb, h);
    chiaki_session_set_event_cb(&h->session, bridge_event_cb, h);

    return h;
}

int michadame_ps5_session_start_local(
    MichadamePs5Session *handle,
    char *err_buf,
    size_t err_cap
) {
    if (!handle) {
        write_err(err_buf, err_cap, "Null session handle");
        return -1;
    }
    pthread_mutex_lock(&handle->lifecycle_mu);
    if (handle->stop_requested) {
        pthread_mutex_unlock(&handle->lifecycle_mu);
        write_err(err_buf, err_cap, "Session canceled");
        return -1;
    }
    ChiakiErrorCode err = chiaki_session_start(&handle->session);
    if (err == CHIAKI_ERR_SUCCESS) {
        handle->session_started = true;
    }
    pthread_mutex_unlock(&handle->lifecycle_mu);
    if (err != CHIAKI_ERR_SUCCESS) {
        char msg[160];
        snprintf(msg, sizeof(msg), "Chiaki session start failed: %s", chiaki_error_string(err));
        write_err(err_buf, err_cap, msg);
        return (int)err;
    }
    return 0;
}

int michadame_ps5_session_connect_psn_and_start(
    MichadamePs5Session *handle,
    const char *psn_duid_hex,
    char *err_buf,
    size_t err_cap
) {
    if (!handle || !handle->holepunch_session || !psn_duid_hex) {
        write_err(err_buf, err_cap, "Invalid PSN session state");
        return -1;
    }

    uint8_t duid_bytes[32];
    if (!hex_to_bytes(psn_duid_hex, duid_bytes, sizeof(duid_bytes))) {
        write_err(err_buf, err_cap, "Invalid PS5 PSN device UID hex");
        return -1;
    }

    pthread_mutex_lock(&handle->lifecycle_mu);
    if (handle->stop_requested) {
        pthread_mutex_unlock(&handle->lifecycle_mu);
        write_err(err_buf, err_cap, "PSN connection canceled");
        return -1;
    }
    handle->holepunch_in_flight = true;
    pthread_mutex_unlock(&handle->lifecycle_mu);

    ChiakiErrorCode err = chiaki_holepunch_upnp_discover(handle->holepunch_session);
    if (err == CHIAKI_ERR_SUCCESS) {
        err = chiaki_holepunch_session_create(handle->holepunch_session);
    }
    if (err == CHIAKI_ERR_SUCCESS) {
        err = holepunch_session_create_offer(handle->holepunch_session);
    }
    if (err == CHIAKI_ERR_SUCCESS) {
        err = chiaki_holepunch_session_start(
            handle->holepunch_session,
            duid_bytes,
            CHIAKI_HOLEPUNCH_CONSOLE_TYPE_PS5
        );
    }
    if (err == CHIAKI_ERR_SUCCESS) {
        err = chiaki_holepunch_session_punch_hole(
            handle->holepunch_session,
            CHIAKI_HOLEPUNCH_PORT_TYPE_CTRL
        );
    }

    pthread_mutex_lock(&handle->lifecycle_mu);
    handle->holepunch_in_flight = false;
    bool stopped = handle->stop_requested;
    if (err == CHIAKI_ERR_SUCCESS && !stopped) {
        err = chiaki_session_start(&handle->session);
        if (err == CHIAKI_ERR_SUCCESS) {
            handle->session_started = true;
        }
    }
    pthread_mutex_unlock(&handle->lifecycle_mu);

    if (stopped) {
        write_err(err_buf, err_cap, "PSN connection canceled");
        return -1;
    }
    if (err != CHIAKI_ERR_SUCCESS) {
        char msg[192];
        snprintf(msg, sizeof(msg), "PSN remote connection failed: %s", chiaki_error_string(err));
        write_err(err_buf, err_cap, msg);
        return (int)err;
    }
    return 0;
}

void michadame_ps5_session_stop(MichadamePs5Session *handle) {
    if (!handle) {
        return;
    }
    pthread_mutex_lock(&handle->lifecycle_mu);
    handle->stop_requested = true;
    bool hp = handle->holepunch_in_flight;
    bool started = handle->session_started;
    pthread_mutex_unlock(&handle->lifecycle_mu);

    if (hp && handle->holepunch_session) {
        chiaki_holepunch_main_thread_cancel(handle->holepunch_session, true);
    }
    if (started) {
        chiaki_session_stop(&handle->session);
    }
}

void michadame_ps5_session_destroy(MichadamePs5Session *handle) {
    if (!handle) {
        return;
    }
    michadame_ps5_session_stop(handle);

    pthread_mutex_lock(&handle->lifecycle_mu);
    bool started = handle->session_started;
    bool initialized = handle->session_initialized;
    pthread_mutex_unlock(&handle->lifecycle_mu);

    if (started) {
        chiaki_session_join(&handle->session);
    }
    if (initialized) {
        chiaki_session_fini(&handle->session);
        chiaki_opus_decoder_fini(&handle->opus_decoder);
    }
    pthread_mutex_destroy(&handle->lifecycle_mu);
    free(handle);
}

int michadame_ps5_session_set_login_pin(MichadamePs5Session *handle, const char *pin) {
    if (!handle || !pin) {
        return -1;
    }
    return (int)chiaki_session_set_login_pin(&handle->session, (const uint8_t *)pin, strlen(pin));
}

int michadame_ps5_session_keyboard_set_text(MichadamePs5Session *handle, const char *text) {
    if (!handle || !text) {
        return -1;
    }
    return (int)chiaki_session_keyboard_set_text(&handle->session, text);
}

int michadame_ps5_session_keyboard_accept(MichadamePs5Session *handle) {
    if (!handle) {
        return -1;
    }
    return (int)chiaki_session_keyboard_accept(&handle->session);
}

int michadame_ps5_session_keyboard_reject(MichadamePs5Session *handle) {
    if (!handle) {
        return -1;
    }
    return (int)chiaki_session_keyboard_reject(&handle->session);
}

int michadame_ps5_session_go_home(MichadamePs5Session *handle) {
    if (!handle) {
        return -1;
    }
    return (int)chiaki_session_go_home(&handle->session);
}

int michadame_ps5_session_goto_bed(MichadamePs5Session *handle) {
    if (!handle) {
        return -1;
    }
    return (int)chiaki_session_goto_bed(&handle->session);
}

int michadame_ps5_session_request_idr(MichadamePs5Session *handle) {
    if (!handle) {
        return -1;
    }
    return (int)chiaki_session_request_idr(&handle->session);
}

/* ===========================
 * DualSense / DualSense Edge 1:1 Manager (SDL2)
 * =========================== */

typedef struct {
    Uint8 ucEnableBits1;
    Uint8 ucEnableBits2;
    Uint8 ucRumbleRight;
    Uint8 ucRumbleLeft;
    Uint8 ucHeadphoneVolume;
    Uint8 ucSpeakerVolume;
    Uint8 ucMicrophoneVolume;
    Uint8 ucAudioEnableBits;
    Uint8 ucMicLightMode;
    Uint8 ucAudioMuteBits;
    Uint8 rgucRightTriggerEffect[11];
    Uint8 rgucLeftTriggerEffect[11];
    Uint8 rgucUnknown1[6];
    Uint8 ucEnableBits3;
    Uint8 rgucUnknown2[2];
    Uint8 ucLedAnim;
    Uint8 ucLedBrightness;
    Uint8 ucPadLights;
    Uint8 ucLedRed;
    Uint8 ucLedGreen;
    Uint8 ucLedBlue;
} DS5EffectsState;

typedef struct {
    bool active;
    int touchpad;
    int finger;
    uint8_t chiaki_id;
} DsTouchSlot;

struct MichadameDualSense {
    bool sdl_gc_initialized;
    bool sdl_audio_initialized;

    SDL_GameController *controller;
    SDL_JoystickID instance_id;
    bool is_edge;
    bool has_led;
    uint16_t vendor_id;
    uint16_t product_id;
    uint16_t firmware_version;
    char name[128];

    ChiakiControllerState state;
    ChiakiOrientationTracker orient_tracker;
    ChiakiAccelNewZero accel_zero;
    ChiakiAccelNewZero real_accel;
    DsTouchSlot touch_slots[CHIAKI_CONTROLLER_TOUCHES_MAX];

    int8_t ps5_rumble_intensity;  /* -1 = off, 0x00 = strong, 0x02 = medium, 0x03 = weak */
    int8_t ps5_trigger_intensity; /* -1 = off, 0x00 = strong, 0x60 = medium, 0x90 = weak */
    uint8_t led_color[3];
    bool has_led_color;
    uint8_t player_index;
    bool has_player_index;

    /* DualSense USB 4-channel 48kHz audio haptics */
    bool haptics_wanted;
    SDL_AudioDeviceID haptics_device;
    SDL_AudioCVT haptics_cvt;
    bool haptics_cvt_ready;
    uint8_t *haptics_buf;
    size_t haptics_buf_cap;
    uint64_t last_haptics_probe_ms;
    uint64_t last_bt_rumble_ms;
    bool bt_rumble_active;
};

static uint8_t ds_intensity_byte(const MichadameDualSense *ds) {
    uint8_t trig = (ds->ps5_trigger_intensity < 0) ? 0xF0u : (uint8_t)ds->ps5_trigger_intensity;
    uint8_t rumb = (ds->ps5_rumble_intensity < 0) ? 0x0Fu : (uint8_t)ds->ps5_rumble_intensity;
    return (uint8_t)(trig | rumb);
}

static bool is_ps5_vid_pid(uint16_t vid, uint16_t pid) {
    return vid == PS5_SONY_VENDOR_ID &&
           (pid == PS5_DUALSENSE_PRODUCT_ID || pid == PS5_DUALSENSE_EDGE_PRODUCT_ID);
}

static bool ds_is_wireless(const MichadameDualSense *ds) {
    if (!ds || !ds->controller) {
        return false;
    }
    SDL_Joystick *js = SDL_GameControllerGetJoystick(ds->controller);
    if (!js) {
        return false;
    }
    switch (SDL_JoystickCurrentPowerLevel(js)) {
        case SDL_JOYSTICK_POWER_EMPTY:
        case SDL_JOYSTICK_POWER_LOW:
        case SDL_JOYSTICK_POWER_MEDIUM:
        case SDL_JOYSTICK_POWER_FULL:
            return true;
        default:
            return false;
    }
}

static void ds_close_haptics_device(MichadameDualSense *ds) {
    if (ds->haptics_device > 0) {
        SDL_CloseAudioDevice(ds->haptics_device);
        ds->haptics_device = 0;
    }
}

static void ds_try_open_haptics_device(MichadameDualSense *ds) {
    if (!ds || !ds->haptics_wanted || !ds->sdl_audio_initialized || ds->haptics_device > 0) {
        return;
    }
    if (!ds->controller || ds_is_wireless(ds)) {
        return;
    }
    if (!ds->haptics_cvt_ready) {
        if (SDL_BuildAudioCVT(&ds->haptics_cvt, AUDIO_S16SYS, 4, 3000, AUDIO_S16SYS, 4, 48000) < 0) {
            return;
        }
        ds->haptics_cvt_ready = true;
    }

    int count = SDL_GetNumAudioDevices(0);
    for (int i = 0; i < count; i++) {
        const char *dev_name = SDL_GetAudioDeviceName(i, 0);
        if (!dev_name) {
            continue;
        }
        if (!strstr(dev_name, "DualSense") && !strstr(dev_name, "Wireless Controller")) {
            continue;
        }
        SDL_AudioSpec want, have;
        SDL_zero(want);
        want.freq = 48000;
        want.format = AUDIO_S16SYS;
        want.channels = 4;
        want.samples = 480;
        want.callback = NULL;

        SDL_AudioDeviceID dev = SDL_OpenAudioDevice(dev_name, 0, &want, &have, 0);
        if (dev > 0) {
            SDL_PauseAudioDevice(dev, 0);
            ds->haptics_device = dev;
            const uint8_t clear_effect[10] = {0};
            michadame_ds_set_trigger_effects(ds, 0x05, clear_effect, 0x05, clear_effect);
            return;
        }
    }
}

static void ds_close_controller(MichadameDualSense *ds) {
    if (!ds) {
        return;
    }
    ds_close_haptics_device(ds);
    if (ds->controller) {
        michadame_ds_clear_effects(ds);
        SDL_GameControllerClose(ds->controller);
        ds->controller = NULL;
    }
    ds->instance_id = -1;
    ds->is_edge = false;
    ds->has_led = false;
    ds->vendor_id = 0;
    ds->product_id = 0;
    ds->firmware_version = 0;
    ds->name[0] = '\0';
    memset(ds->touch_slots, 0, sizeof(ds->touch_slots));
    chiaki_controller_state_set_idle(&ds->state);
    chiaki_orientation_tracker_init(&ds->orient_tracker);
    chiaki_accel_new_zero_set_inactive(&ds->accel_zero, false);
    chiaki_accel_new_zero_set_inactive(&ds->real_accel, true);
}

static void ds_try_attach(MichadameDualSense *ds) {
    if (!ds || !ds->sdl_gc_initialized) {
        return;
    }
    if (ds->controller) {
        if (!SDL_GameControllerGetAttached(ds->controller)) {
            ds_close_controller(ds);
        } else {
            return;
        }
    }

    int num = SDL_NumJoysticks();
    for (int i = 0; i < num; i++) {
        if (!SDL_IsGameController(i)) {
            continue;
        }
        uint16_t vid = SDL_JoystickGetDeviceVendor(i);
        uint16_t pid = SDL_JoystickGetDeviceProduct(i);
        if (!is_ps5_vid_pid(vid, pid)) {
            continue;
        }

        SDL_GameController *gc = SDL_GameControllerOpen(i);
        if (!gc) {
            continue;
        }
        SDL_Joystick *js = SDL_GameControllerGetJoystick(gc);
        if (!js || SDL_JoystickNumButtons(js) == 0) {
            SDL_GameControllerClose(gc);
            continue;
        }
        uint16_t actual_vid = SDL_GameControllerGetVendor(gc);
        uint16_t actual_pid = SDL_GameControllerGetProduct(gc);
        if (!is_ps5_vid_pid(actual_vid, actual_pid)) {
            SDL_GameControllerClose(gc);
            continue;
        }

        if (SDL_GameControllerHasSensor(gc, SDL_SENSOR_ACCEL)) {
            SDL_GameControllerSetSensorEnabled(gc, SDL_SENSOR_ACCEL, SDL_TRUE);
        }
        if (SDL_GameControllerHasSensor(gc, SDL_SENSOR_GYRO)) {
            SDL_GameControllerSetSensorEnabled(gc, SDL_SENSOR_GYRO, SDL_TRUE);
        }

        ds->controller = gc;
        ds->instance_id = SDL_JoystickInstanceID(js);
        ds->vendor_id = actual_vid;
        ds->product_id = actual_pid;
        ds->is_edge = (actual_pid == PS5_DUALSENSE_EDGE_PRODUCT_ID);
        ds->has_led = SDL_GameControllerHasLED(gc) == SDL_TRUE;
        ds->firmware_version = SDL_GameControllerGetFirmwareVersion(gc);

        const char *jname = SDL_JoystickName(js);
        copy_str_safe(
            ds->name,
            sizeof(ds->name),
            (jname && jname[0] != '\0') ? jname
                : (ds->is_edge ? "DualSense Edge Wireless Controller"
                               : "DualSense Wireless Controller")
        );

        chiaki_controller_state_set_idle(&ds->state);
        memset(ds->touch_slots, 0, sizeof(ds->touch_slots));
        chiaki_orientation_tracker_init(&ds->orient_tracker);
        chiaki_accel_new_zero_set_inactive(&ds->accel_zero, false);
        chiaki_accel_new_zero_set_inactive(&ds->real_accel, true);

        if (ds->has_led_color) {
            michadame_ds_set_led(ds, ds->led_color[0], ds->led_color[1], ds->led_color[2]);
        }
        if (ds->has_player_index) {
            michadame_ds_set_player_index(ds, ds->player_index);
        }
        if (ds->haptics_wanted) {
            ds_try_open_haptics_device(ds);
        }
        return;
    }
}

MichadameDualSense *michadame_ds_create(void) {
    michadame_ps5_lib_init();

    MichadameDualSense *ds = (MichadameDualSense *)calloc(1, sizeof(MichadameDualSense));
    if (!ds) {
        return NULL;
    }
    ds->instance_id = -1;
    ds->ps5_rumble_intensity = 0x00;  /* Strong by default */
    ds->ps5_trigger_intensity = 0x00; /* Strong by default */

    chiaki_controller_state_set_idle(&ds->state);
    chiaki_orientation_tracker_init(&ds->orient_tracker);
    chiaki_accel_new_zero_set_inactive(&ds->accel_zero, false);
    chiaki_accel_new_zero_set_inactive(&ds->real_accel, true);

    SDL_SetMainReady();
    SDL_SetHint(SDL_HINT_JOYSTICK_HIDAPI_PS4_RUMBLE, "1");
    SDL_SetHint(SDL_HINT_JOYSTICK_HIDAPI_PS5_RUMBLE, "1");
    SDL_SetHint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, "1");
    SDL_SetHint(SDL_HINT_GAMECONTROLLER_USE_BUTTON_LABELS, "0");

    if (SDL_InitSubSystem(SDL_INIT_GAMECONTROLLER) == 0) {
        ds->sdl_gc_initialized = true;
    }

    /* Prefer PipeWire for 4-channel DualSense USB haptics; fall back cleanly */
    SDL_SetHint("SDL_AUDIODRIVER", "pipewire");
    if (SDL_InitSubSystem(SDL_INIT_AUDIO) == 0) {
        ds->sdl_audio_initialized = true;
    } else {
        SDL_SetHint("SDL_AUDIODRIVER", "");
        if (SDL_InitSubSystem(SDL_INIT_AUDIO) == 0) {
            ds->sdl_audio_initialized = true;
        }
    }

    ds_try_attach(ds);
    return ds;
}

void michadame_ds_destroy(MichadameDualSense *ds) {
    if (!ds) {
        return;
    }
    ds_close_controller(ds);
    if (ds->haptics_buf) {
        free(ds->haptics_buf);
        ds->haptics_buf = NULL;
        ds->haptics_buf_cap = 0;
    }
    if (ds->sdl_audio_initialized) {
        SDL_QuitSubSystem(SDL_INIT_AUDIO);
    }
    if (ds->sdl_gc_initialized) {
        SDL_QuitSubSystem(SDL_INIT_GAMECONTROLLER);
    }
    free(ds);
}

static bool ds_handle_button(MichadameDualSense *ds, const SDL_ControllerButtonEvent *ev) {
    ChiakiControllerButton ps_btn;
    switch (ev->button) {
        case SDL_CONTROLLER_BUTTON_A: ps_btn = CHIAKI_CONTROLLER_BUTTON_CROSS; break;
        case SDL_CONTROLLER_BUTTON_B: ps_btn = CHIAKI_CONTROLLER_BUTTON_MOON; break;
        case SDL_CONTROLLER_BUTTON_X: ps_btn = CHIAKI_CONTROLLER_BUTTON_BOX; break;
        case SDL_CONTROLLER_BUTTON_Y: ps_btn = CHIAKI_CONTROLLER_BUTTON_PYRAMID; break;
        case SDL_CONTROLLER_BUTTON_DPAD_LEFT: ps_btn = CHIAKI_CONTROLLER_BUTTON_DPAD_LEFT; break;
        case SDL_CONTROLLER_BUTTON_DPAD_RIGHT: ps_btn = CHIAKI_CONTROLLER_BUTTON_DPAD_RIGHT; break;
        case SDL_CONTROLLER_BUTTON_DPAD_UP: ps_btn = CHIAKI_CONTROLLER_BUTTON_DPAD_UP; break;
        case SDL_CONTROLLER_BUTTON_DPAD_DOWN: ps_btn = CHIAKI_CONTROLLER_BUTTON_DPAD_DOWN; break;
        case SDL_CONTROLLER_BUTTON_LEFTSHOULDER: ps_btn = CHIAKI_CONTROLLER_BUTTON_L1; break;
        case SDL_CONTROLLER_BUTTON_RIGHTSHOULDER: ps_btn = CHIAKI_CONTROLLER_BUTTON_R1; break;
        case SDL_CONTROLLER_BUTTON_LEFTSTICK: ps_btn = CHIAKI_CONTROLLER_BUTTON_L3; break;
        case SDL_CONTROLLER_BUTTON_RIGHTSTICK: ps_btn = CHIAKI_CONTROLLER_BUTTON_R3; break;
        case SDL_CONTROLLER_BUTTON_START: ps_btn = CHIAKI_CONTROLLER_BUTTON_OPTIONS; break;
        case SDL_CONTROLLER_BUTTON_BACK: ps_btn = CHIAKI_CONTROLLER_BUTTON_SHARE; break;
        case SDL_CONTROLLER_BUTTON_GUIDE: ps_btn = CHIAKI_CONTROLLER_BUTTON_PS; break;
        case SDL_CONTROLLER_BUTTON_TOUCHPAD: ps_btn = CHIAKI_CONTROLLER_BUTTON_TOUCHPAD; break;
        /* Ignore mic button (MISC1) and unmapped Edge paddles */
        default:
            return false;
    }
    if (ev->type == SDL_CONTROLLERBUTTONDOWN) {
        ds->state.buttons |= ps_btn;
    } else {
        ds->state.buttons &= ~ps_btn;
    }
    return true;
}

static bool ds_handle_axis(MichadameDualSense *ds, const SDL_ControllerAxisEvent *ev) {
    switch (ev->axis) {
        case SDL_CONTROLLER_AXIS_TRIGGERLEFT:
            ds->state.l2_state = (ev->value > 0) ? (uint8_t)(ev->value >> 7) : 0;
            return true;
        case SDL_CONTROLLER_AXIS_TRIGGERRIGHT:
            ds->state.r2_state = (ev->value > 0) ? (uint8_t)(ev->value >> 7) : 0;
            return true;
        case SDL_CONTROLLER_AXIS_LEFTX:
            ds->state.left_x = ev->value;
            return true;
        case SDL_CONTROLLER_AXIS_LEFTY:
            ds->state.left_y = ev->value;
            return true;
        case SDL_CONTROLLER_AXIS_RIGHTX:
            ds->state.right_x = ev->value;
            return true;
        case SDL_CONTROLLER_AXIS_RIGHTY:
            ds->state.right_y = ev->value;
            return true;
        default:
            return false;
    }
}

static bool ds_handle_sensor(MichadameDualSense *ds, const SDL_ControllerSensorEvent *ev) {
    uint32_t ts_us = (uint32_t)(ev->timestamp * 1000u);
    if (ev->sensor == SDL_SENSOR_ACCEL) {
        float ax = ev->data[0] / SDL_STANDARD_GRAVITY;
        float ay = ev->data[1] / SDL_STANDARD_GRAVITY;
        float az = ev->data[2] / SDL_STANDARD_GRAVITY;
        chiaki_accel_new_zero_set_active(&ds->real_accel, ax, ay, az, true);
        chiaki_orientation_tracker_update(
            &ds->orient_tracker,
            ds->state.gyro_x, ds->state.gyro_y, ds->state.gyro_z,
            ax, ay, az,
            &ds->accel_zero,
            false,
            ts_us
        );
    } else if (ev->sensor == SDL_SENSOR_GYRO) {
        chiaki_orientation_tracker_update(
            &ds->orient_tracker,
            ev->data[0], ev->data[1], ev->data[2],
            ds->state.accel_x, ds->state.accel_y, ds->state.accel_z,
            &ds->accel_zero,
            true,
            ts_us
        );
    } else {
        return false;
    }
    chiaki_orientation_tracker_apply_to_controller_state(&ds->orient_tracker, &ds->state);
    return true;
}

static bool ds_handle_touchpad(MichadameDualSense *ds, const SDL_ControllerTouchpadEvent *ev) {
    float fx = ev->x;
    float fy = ev->y;
    if (fx < 0.0f) fx = 0.0f;
    if (fx > 1.0f) fx = 1.0f;
    if (fy < 0.0f) fy = 0.0f;
    if (fy > 1.0f) fy = 1.0f;
    uint16_t px = (uint16_t)(fx * PS5_TOUCHPAD_MAX_X);
    uint16_t py = (uint16_t)(fy * PS5_TOUCHPAD_MAX_Y);

    int existing = -1;
    int free_slot = -1;
    for (int i = 0; i < CHIAKI_CONTROLLER_TOUCHES_MAX; i++) {
        if (ds->touch_slots[i].active &&
            ds->touch_slots[i].touchpad == ev->touchpad &&
            ds->touch_slots[i].finger == ev->finger) {
            existing = i;
        } else if (!ds->touch_slots[i].active && free_slot < 0) {
            free_slot = i;
        }
    }

    if (ev->type == SDL_CONTROLLERTOUCHPADDOWN) {
        if (existing >= 0) {
            chiaki_controller_state_set_touch_pos(&ds->state, ds->touch_slots[existing].chiaki_id, px, py);
            return true;
        }
        if (free_slot < 0) {
            return false;
        }
        int8_t cid = chiaki_controller_state_start_touch(&ds->state, px, py);
        if (cid < 0) {
            return false;
        }
        ds->touch_slots[free_slot].active = true;
        ds->touch_slots[free_slot].touchpad = ev->touchpad;
        ds->touch_slots[free_slot].finger = ev->finger;
        ds->touch_slots[free_slot].chiaki_id = (uint8_t)cid;
        return true;
    } else if (ev->type == SDL_CONTROLLERTOUCHPADMOTION) {
        if (existing < 0) {
            return false;
        }
        chiaki_controller_state_set_touch_pos(&ds->state, ds->touch_slots[existing].chiaki_id, px, py);
        return true;
    } else if (ev->type == SDL_CONTROLLERTOUCHPADUP) {
        if (existing < 0) {
            return false;
        }
        chiaki_controller_state_stop_touch(&ds->state, ds->touch_slots[existing].chiaki_id);
        ds->touch_slots[existing].active = false;
        return true;
    }
    return false;
}

void michadame_ds_poll(
    MichadameDualSense *ds,
    MichadamePs5Session *active_session,
    MichadameDualSenseInfo *out_info
) {
    if (!ds) {
        if (out_info) {
            memset(out_info, 0, sizeof(*out_info));
        }
        return;
    }

    bool state_changed = false;
    if (ds->sdl_gc_initialized) {
        SDL_Event ev;
        while (SDL_PollEvent(&ev)) {
            switch (ev.type) {
                case SDL_JOYDEVICEADDED:
                case SDL_CONTROLLERDEVICEADDED:
                    ds_try_attach(ds);
                    break;
                case SDL_JOYDEVICEREMOVED:
                    if (ds->controller && ev.jdevice.which == ds->instance_id) {
                        ds_close_controller(ds);
                        state_changed = true;
                        ds_try_attach(ds);
                    }
                    break;
                case SDL_CONTROLLERDEVICEREMOVED:
                    if (ds->controller && ev.cdevice.which == ds->instance_id) {
                        ds_close_controller(ds);
                        state_changed = true;
                        ds_try_attach(ds);
                    }
                    break;
                case SDL_AUDIODEVICEADDED:
                    if (!ev.adevice.iscapture && ds->haptics_wanted && ds->haptics_device == 0) {
                        ds_try_open_haptics_device(ds);
                    }
                    break;
                case SDL_CONTROLLERBUTTONDOWN:
                case SDL_CONTROLLERBUTTONUP:
                    if (ds->controller && ev.cbutton.which == ds->instance_id) {
                        state_changed |= ds_handle_button(ds, &ev.cbutton);
                    }
                    break;
                case SDL_CONTROLLERAXISMOTION:
                    if (ds->controller && ev.caxis.which == ds->instance_id) {
                        state_changed |= ds_handle_axis(ds, &ev.caxis);
                    }
                    break;
                case SDL_CONTROLLERSENSORUPDATE:
                    if (ds->controller && ev.csensor.which == ds->instance_id) {
                        state_changed |= ds_handle_sensor(ds, &ev.csensor);
                    }
                    break;
                case SDL_CONTROLLERTOUCHPADDOWN:
                case SDL_CONTROLLERTOUCHPADMOTION:
                case SDL_CONTROLLERTOUCHPADUP:
                    if (ds->controller && ev.ctouchpad.which == ds->instance_id) {
                        state_changed |= ds_handle_touchpad(ds, &ev.ctouchpad);
                    }
                    break;
                default:
                    break;
            }
        }

        /* Catch hotplug transitions even if SDL missed an event */
        ds_try_attach(ds);

        if (ds->haptics_wanted && ds->controller && ds->haptics_device == 0 && !ds_is_wireless(ds)) {
            uint64_t now_ms = chiaki_time_now_monotonic_ms();
            if (now_ms - ds->last_haptics_probe_ms >= 1500) {
                ds->last_haptics_probe_ms = now_ms;
                ds_try_open_haptics_device(ds);
            }
        }
    }

    if (active_session && state_changed) {
        chiaki_session_set_controller_state(&active_session->session, &ds->state);
    }

    if (out_info) {
        memset(out_info, 0, sizeof(*out_info));
        out_info->connected = (ds->controller != NULL);
        out_info->is_edge = ds->is_edge;
        out_info->is_wireless = ds_is_wireless(ds);
        out_info->haptics_audio_connected = (ds->haptics_device > 0);
        out_info->vendor_id = ds->vendor_id;
        out_info->product_id = ds->product_id;
        copy_str_safe(out_info->name, sizeof(out_info->name), ds->name);
    }
}

void michadame_ds_set_rumble(MichadameDualSense *ds, uint8_t left, uint8_t right) {
    if (!ds || !ds->controller || ds->ps5_rumble_intensity < 0) {
        return;
    }
    DS5EffectsState st;
    SDL_zero(st);
    if (!ds->is_edge && ds->firmware_version != 0 && ds->firmware_version < 0x0224) {
        st.ucEnableBits1 |= 0x01;
        st.ucRumbleLeft = (uint8_t)(left >> 1);
        st.ucRumbleRight = (uint8_t)(right >> 1);
    } else {
        st.ucEnableBits3 |= 0x04;
        st.ucRumbleLeft = left;
        st.ucRumbleRight = right;
    }
    st.rgucUnknown1[4] = ds_intensity_byte(ds);
    st.ucEnableBits2 |= 0x40;
    st.ucEnableBits1 |= 0x02;
    SDL_GameControllerSendEffect(ds->controller, &st, sizeof(st));
}

void michadame_ds_set_trigger_effects(
    MichadameDualSense *ds,
    uint8_t type_left,
    const uint8_t left[10],
    uint8_t type_right,
    const uint8_t right[10]
) {
    if (!ds || !ds->controller || !left || !right) {
        return;
    }
    if (ds->ps5_trigger_intensity < 0 && type_left != 0x05 && type_right != 0x05) {
        return;
    }
    DS5EffectsState st;
    SDL_zero(st);
    st.rgucUnknown1[4] = ds_intensity_byte(ds);
    st.ucEnableBits2 |= 0x40;
    st.ucEnableBits1 |= (0x04 | 0x08);
    st.rgucLeftTriggerEffect[0] = type_left;
    memcpy(st.rgucLeftTriggerEffect + 1, left, 10);
    st.rgucRightTriggerEffect[0] = type_right;
    memcpy(st.rgucRightTriggerEffect + 1, right, 10);
    SDL_GameControllerSendEffect(ds->controller, &st, sizeof(st));
}

void michadame_ds_set_led(MichadameDualSense *ds, uint8_t r, uint8_t g, uint8_t b) {
    if (!ds) {
        return;
    }
    ds->led_color[0] = r;
    ds->led_color[1] = g;
    ds->led_color[2] = b;
    ds->has_led_color = true;
    if (ds->controller && ds->has_led) {
        SDL_GameControllerSetLED(ds->controller, r, g, b);
    }
}

void michadame_ds_set_player_index(MichadameDualSense *ds, uint8_t player_index) {
    if (!ds) {
        return;
    }
    ds->player_index = player_index;
    ds->has_player_index = true;
    if (ds->controller) {
        SDL_GameControllerSetPlayerIndex(ds->controller, (int)player_index);
    }
}

void michadame_ds_set_haptic_intensity(MichadameDualSense *ds, int intensity) {
    if (!ds) {
        return;
    }
    switch (intensity) {
        case Off: ds->ps5_rumble_intensity = -1; break;
        case Strong: ds->ps5_rumble_intensity = 0x00; break;
        case Medium: ds->ps5_rumble_intensity = 0x02; break;
        case Weak: ds->ps5_rumble_intensity = 0x03; break;
        default: ds->ps5_rumble_intensity = 0x00; break;
    }
}

void michadame_ds_set_trigger_intensity(MichadameDualSense *ds, int intensity) {
    if (!ds) {
        return;
    }
    switch (intensity) {
        case Off: ds->ps5_trigger_intensity = -1; break;
        case Strong: ds->ps5_trigger_intensity = 0x00; break;
        case Medium: ds->ps5_trigger_intensity = 0x60; break;
        case Weak: ds->ps5_trigger_intensity = 0x90; break;
        default: ds->ps5_trigger_intensity = 0x00; break;
    }
}

void michadame_ds_reset_motion(MichadameDualSense *ds) {
    if (!ds) {
        return;
    }
    chiaki_accel_new_zero_set_active(
        &ds->accel_zero,
        ds->real_accel.accel_x,
        ds->real_accel.accel_y,
        ds->real_accel.accel_z,
        false
    );
    chiaki_orientation_tracker_init(&ds->orient_tracker);
    chiaki_orientation_tracker_update(
        &ds->orient_tracker,
        ds->state.gyro_x,
        ds->state.gyro_y,
        ds->state.gyro_z,
        ds->real_accel.accel_x,
        ds->real_accel.accel_y,
        ds->real_accel.accel_z,
        &ds->accel_zero,
        false,
        (uint32_t)chiaki_time_now_monotonic_us()
    );
    chiaki_orientation_tracker_apply_to_controller_state(&ds->orient_tracker, &ds->state);
}

void michadame_ds_clear_effects(MichadameDualSense *ds) {
    if (!ds || !ds->controller) {
        return;
    }
    const uint8_t clear_effect[10] = {0};
    michadame_ds_set_trigger_effects(ds, 0x05, clear_effect, 0x05, clear_effect);
    michadame_ds_set_rumble(ds, 0, 0);
}

void michadame_ds_start_haptics(MichadameDualSense *ds) {
    if (!ds) {
        return;
    }
    ds->haptics_wanted = true;
    ds->last_haptics_probe_ms = 0;
    ds_try_open_haptics_device(ds);
}

void michadame_ds_stop_haptics(MichadameDualSense *ds) {
    if (!ds) {
        return;
    }
    ds->haptics_wanted = false;
    ds_close_haptics_device(ds);
    michadame_ds_clear_effects(ds);
}

void michadame_ds_push_haptics(MichadameDualSense *ds, const uint8_t *buf, size_t buf_size) {
    if (!ds || !buf || buf_size == 0 || (buf_size % (2 * sizeof(int16_t))) != 0) {
        return;
    }
    if (ds->ps5_rumble_intensity < 0) {
        return;
    }

    /* Wired DualSense USB 4-channel haptics audio path */
    if (ds->haptics_device > 0 && ds->haptics_cvt_ready) {
        size_t remix_len = buf_size * 2;
        size_t needed = remix_len * (size_t)ds->haptics_cvt.len_mult;
        if (needed > ds->haptics_buf_cap) {
            uint8_t *grown = (uint8_t *)realloc(ds->haptics_buf, needed);
            if (!grown) {
                return;
            }
            ds->haptics_buf = grown;
            ds->haptics_buf_cap = needed;
        }
        /* Place stereo haptic PCM into channels 3 & 4 of 4-channel frame */
        for (size_t i = 0; i < buf_size; i += 4) {
            memset(ds->haptics_buf + i * 2, 0, 4);
            memcpy(ds->haptics_buf + (i * 2) + 4, buf + i, 4);
        }
        SDL_AudioCVT cvt = ds->haptics_cvt;
        cvt.buf = ds->haptics_buf;
        cvt.len = (int)remix_len;
        if (SDL_ConvertAudio(&cvt) != 0) {
            return;
        }
        const Uint32 max_queued = (Uint32)(480 * 4 * sizeof(int16_t) * 6);
        if (SDL_GetQueuedAudioSize(ds->haptics_device) > max_queued) {
            SDL_ClearQueuedAudio(ds->haptics_device);
        }
        SDL_QueueAudio(ds->haptics_device, cvt.buf, (Uint32)cvt.len_cvt);
        return;
    }

    /* Bluetooth fallback: translate DualSense haptic waveform into smooth motor rumble */
    if (ds->controller) {
        uint64_t now_ms = chiaki_time_now_monotonic_ms();
        if (now_ms - ds->last_bt_rumble_ms < 25) {
            return;
        }
        size_t frames = buf_size / (2 * sizeof(int16_t));
        uint32_t sum_l = 0, sum_r = 0;
        for (size_t i = 0; i < frames; i++) {
            int16_t l = 0, r = 0;
            memcpy(&l, buf + i * 4, sizeof(int16_t));
            memcpy(&r, buf + i * 4 + 2, sizeof(int16_t));
            sum_l += (uint32_t)(l < 0 ? -(int32_t)l : (int32_t)l);
            sum_r += (uint32_t)(r < 0 ? -(int32_t)r : (int32_t)r);
        }
        uint32_t avg_l = frames ? (sum_l / (uint32_t)frames) : 0;
        uint32_t avg_r = frames ? (sum_r / (uint32_t)frames) : 0;
        uint8_t left = (avg_l > 512) ? (uint8_t)((avg_l >> 6) > 255 ? 255 : (avg_l >> 6)) : 0;
        uint8_t right = (avg_r > 512) ? (uint8_t)((avg_r >> 6) > 255 ? 255 : (avg_r >> 6)) : 0;
        if (left > 0 || right > 0 || ds->bt_rumble_active) {
            ds->last_bt_rumble_ms = now_ms;
            ds->bt_rumble_active = (left > 0 || right > 0);
            michadame_ds_set_rumble(ds, left, right);
        }
    }
}
