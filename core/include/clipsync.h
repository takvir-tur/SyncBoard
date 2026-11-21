#ifndef CLIPSYNC_H
#define CLIPSYNC_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ClipSyncEngine ClipSyncEngine;

typedef struct ClipSyncItem {
    uint8_t kind;        /* 1 = text, 2 = image */
    const uint8_t *data; /* UTF-8 bytes or normalized image bytes */
    size_t data_len;
    char *mime;          /* NUL-terminated MIME string */
    uint64_t timestamp_ms;
} ClipSyncItem;

typedef struct ClipSyncPin {
    uint8_t id[16];
    uint8_t kind;        /* 1 = text, 2 = image */
    const uint8_t *data;
    size_t data_len;
    char *mime;
    uint64_t created_at_ms;
    uint64_t updated_at_ms;
} ClipSyncPin;

typedef struct ClipSyncPinList {
    ClipSyncPin *items;
    size_t count;
} ClipSyncPinList;

ClipSyncEngine *clipsync_init(const char *data_dir, const char *relay_url);
int32_t clipsync_push_local(ClipSyncEngine *engine, uint8_t kind, const uint8_t *data, size_t len);
int32_t clipsync_pop_remote(ClipSyncEngine *engine, ClipSyncItem *out_item);
int32_t clipsync_pin_item(ClipSyncEngine *engine, uint8_t kind, const uint8_t *data, size_t len, uint8_t *out_id_16);
int32_t clipsync_unpin_item(ClipSyncEngine *engine, const uint8_t *id_16);
int32_t clipsync_get_pins(ClipSyncEngine *engine, ClipSyncPinList *out_list);
char *clipsync_get_pairing_uri(ClipSyncEngine *engine);
int32_t clipsync_join_pairing_uri(ClipSyncEngine *engine, const char *uri);
void clipsync_record_echo(ClipSyncEngine *engine, uint8_t kind, const uint8_t *data, size_t len);
int32_t clipsync_wait_event(ClipSyncEngine *engine, uint64_t timeout_ms);
void clipsync_free_item(ClipSyncItem *item);
void clipsync_free_pin_list(ClipSyncPinList *list);
void clipsync_free_string(char *s);
void clipsync_shutdown(ClipSyncEngine *engine);

#ifdef __cplusplus
}
#endif

#endif /* CLIPSYNC_H */
