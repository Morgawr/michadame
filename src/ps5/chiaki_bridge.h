#ifndef MICHADAME_PS5_CHIAKI_BRIDGE_H
#define MICHADAME_PS5_CHIAKI_BRIDGE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MichadamePs5Session MichadamePs5Session;
typedef struct MichadameDualSense MichadameDualSense;

/* Matches ChiakiDiscoveryHostState */
typedef enum {
    MICHADAME_PS5_HOST_UNKNOWN = 0,
    MICHADAME_PS5_HOST_READY = 1,
    MICHADAME_PS5_HOST_STANDBY = 2
} MichadamePs5HostState;

typedef struct {
    int state;
    uint16_t host_request_port;
    char host_addr[64];
    char host_name[128];
    char host_id[32];
    char host_type[32];
    char system_version[32];
    char running_app_titleid[64];
    char running_app_name[128];
    bool is_ps5;
} MichadamePs5DiscoveredHost;

typedef void (*MichadamePs5DiscoveredHostCb)(
    const MichadamePs5DiscoveredHost *host,
    void *user
);

typedef struct {
    char device_name[64];
    char device_uid_hex[65];
    bool remoteplay_enabled;
    bool is_ps5;
} MichadamePs5PsnDevice;

typedef void (*MichadamePs5PsnDeviceCb)(
    const MichadamePs5PsnDevice *dev,
    void *user
);

typedef struct {
    uint8_t server_mac[6];
    char server_nickname[64];
    uint8_t rp_regist_key[16];
    uint8_t rp_key[16];
    uint32_t rp_key_type;
    uint32_t console_pin;
    int target;
} MichadamePs5RegistResult;

typedef struct {
    bool connected;
    bool is_edge;
    bool is_wireless;
    bool haptics_audio_connected;
    uint16_t vendor_id;
    uint16_t product_id;
    char name[128];
} MichadameDualSenseInfo;

/* Session event types forwarded to Rust */
typedef enum {
    MICHADAME_PS5_EV_CONNECTED = 1,
    MICHADAME_PS5_EV_QUIT = 2,
    MICHADAME_PS5_EV_LOGIN_PIN_REQUEST = 3,
    MICHADAME_PS5_EV_REGIST = 4,
    MICHADAME_PS5_EV_NICKNAME = 5,
    MICHADAME_PS5_EV_KEYBOARD_OPEN = 6,
    MICHADAME_PS5_EV_KEYBOARD_TEXT_CHANGE = 7,
    MICHADAME_PS5_EV_KEYBOARD_REMOTE_CLOSE = 8,
    MICHADAME_PS5_EV_RUMBLE = 9,
    MICHADAME_PS5_EV_TRIGGER_EFFECTS = 10,
    MICHADAME_PS5_EV_LED_COLOR = 11,
    MICHADAME_PS5_EV_PLAYER_INDEX = 12,
    MICHADAME_PS5_EV_HAPTIC_INTENSITY = 13,
    MICHADAME_PS5_EV_TRIGGER_INTENSITY = 14,
    MICHADAME_PS5_EV_MOTION_RESET = 15,
    MICHADAME_PS5_EV_HOLEPUNCH = 16,
    MICHADAME_PS5_EV_FEC_FAILURE = 17,
    MICHADAME_PS5_EV_CANT_DISPLAY = 18
} MichadamePs5EventType;

typedef struct {
    int type;
    int quit_reason;
    bool quit_is_error;
    bool flag; /* pin_incorrect / holepunch finished / cant_display / idr_request_sent */
    uint8_t rumble_left;
    uint8_t rumble_right;
    uint8_t trigger_type_left;
    uint8_t trigger_type_right;
    uint8_t trigger_left[10];
    uint8_t trigger_right[10];
    uint8_t led_color[3];
    uint8_t player_index;
    int intensity; /* 0=Strong, 1=Medium, 2=Weak, 3=Off */
    int32_t frame_index;
    const char *text; /* quit reason / nickname / keyboard text (valid only during callback) */
    MichadamePs5RegistResult regist;
} MichadamePs5Event;

typedef bool (*MichadamePs5VideoCb)(
    const uint8_t *buf,
    size_t buf_size,
    int32_t frames_lost,
    bool frame_recovered,
    void *user
);
typedef void (*MichadamePs5AudioSettingsCb)(uint32_t channels, uint32_t rate, void *user);
typedef void (*MichadamePs5AudioFrameCb)(const int16_t *pcm, size_t samples_count, void *user);
typedef void (*MichadamePs5HapticsFrameCb)(const uint8_t *buf, size_t buf_size, void *user);
typedef void (*MichadamePs5EventCb)(const MichadamePs5Event *ev, void *user);
typedef void (*MichadamePs5LogCb)(int level, const char *msg, void *user);

typedef struct {
    const char *host;
    uint8_t regist_key[16];
    uint8_t morning[16];
    /* If psn_oauth_token and psn_duid_hex are non-empty, connect via PSN holepunch */
    const char *psn_oauth_token;
    const char *psn_duid_hex;
    uint8_t psn_account_id[8];
    bool auto_regist;

    uint32_t width;
    uint32_t height;
    uint32_t max_fps;
    uint32_t bitrate_kbps;
    int codec; /* 0 = H264, 1 = H265 */

    bool enable_keyboard;
    bool enable_dualsense;

    MichadamePs5VideoCb video_cb;
    MichadamePs5AudioSettingsCb audio_settings_cb;
    MichadamePs5AudioFrameCb audio_frame_cb;
    MichadamePs5HapticsFrameCb haptics_frame_cb;
    MichadamePs5EventCb event_cb;
    MichadamePs5LogCb log_cb;
    void *user;
} MichadamePs5SessionConfig;

/* Global initialization and helpers */
int michadame_ps5_lib_init(void);
int michadame_ps5_generate_client_duid(char *out_buf, size_t out_cap);

/* Local discovery & wakeup */
int michadame_ps5_discover_once(
    uint32_t timeout_ms,
    MichadamePs5DiscoveredHostCb cb,
    void *user
);
int michadame_ps5_wakeup(const char *host, const uint8_t regist_key[16]);

/* PSN remote device listing */
int michadame_ps5_psn_list_devices(
    const char *psn_oauth_token,
    MichadamePs5PsnDeviceCb cb,
    void *user,
    char *err_buf,
    size_t err_cap
);

/* Local PIN registration */
int michadame_ps5_register_local(
    const char *host,
    bool broadcast,
    const uint8_t psn_account_id[8],
    uint32_t pin,
    uint32_t console_pin,
    MichadamePs5RegistResult *out_result,
    char *err_buf,
    size_t err_cap
);

/* Streaming session lifecycle */
MichadamePs5Session *michadame_ps5_session_create(
    const MichadamePs5SessionConfig *cfg,
    char *err_buf,
    size_t err_cap
);
int michadame_ps5_session_connect_psn_and_start(
    MichadamePs5Session *handle,
    const char *psn_duid_hex,
    char *err_buf,
    size_t err_cap
);
int michadame_ps5_session_start_local(
    MichadamePs5Session *handle,
    char *err_buf,
    size_t err_cap
);
void michadame_ps5_session_stop(MichadamePs5Session *handle);
void michadame_ps5_session_destroy(MichadamePs5Session *handle);

/* Session runtime commands */
int michadame_ps5_session_set_login_pin(MichadamePs5Session *handle, const char *pin);
int michadame_ps5_session_keyboard_set_text(MichadamePs5Session *handle, const char *text);
int michadame_ps5_session_keyboard_accept(MichadamePs5Session *handle);
int michadame_ps5_session_keyboard_reject(MichadamePs5Session *handle);
int michadame_ps5_session_go_home(MichadamePs5Session *handle);
int michadame_ps5_session_goto_bed(MichadamePs5Session *handle);
int michadame_ps5_session_request_idr(MichadamePs5Session *handle);

/* DualSense / DualSense Edge 1:1 controller manager (must be used from a single dedicated thread) */
MichadameDualSense *michadame_ds_create(void);
void michadame_ds_destroy(MichadameDualSense *ds);
void michadame_ds_poll(
    MichadameDualSense *ds,
    MichadamePs5Session *active_session,
    MichadameDualSenseInfo *out_info
);
void michadame_ds_set_rumble(MichadameDualSense *ds, uint8_t left, uint8_t right);
void michadame_ds_set_trigger_effects(
    MichadameDualSense *ds,
    uint8_t type_left,
    const uint8_t left[10],
    uint8_t type_right,
    const uint8_t right[10]
);
void michadame_ds_set_led(MichadameDualSense *ds, uint8_t r, uint8_t g, uint8_t b);
void michadame_ds_set_player_index(MichadameDualSense *ds, uint8_t player_index);
void michadame_ds_set_haptic_intensity(MichadameDualSense *ds, int intensity);
void michadame_ds_set_trigger_intensity(MichadameDualSense *ds, int intensity);
void michadame_ds_reset_motion(MichadameDualSense *ds);
void michadame_ds_clear_effects(MichadameDualSense *ds);
void michadame_ds_start_haptics(MichadameDualSense *ds);
void michadame_ds_stop_haptics(MichadameDualSense *ds);
void michadame_ds_push_haptics(MichadameDualSense *ds, const uint8_t *buf, size_t buf_size);

#ifdef __cplusplus
}
#endif

#endif /* MICHADAME_PS5_CHIAKI_BRIDGE_H */
