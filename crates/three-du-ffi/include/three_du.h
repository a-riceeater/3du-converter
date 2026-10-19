/*
 * three_du.h: C interface to 3du-converter (libthree_du).
 *
 * Ownership: every pointer returned by a tdu_* function belongs to the caller and must be
 * released with the matching tdu_*_free function, exactly once. tdu_version() and
 * tdu_error_message() return borrowed strings that must not be freed.
 * Errors: failing calls return NULL or a nonzero TDU_ERR_* code and, when out_error is not
 * NULL, store a TduError the caller frees with tdu_error_free.
 * Threads: a TduProject is immutable; it may be read and exported from several threads at
 * once. Progress callbacks run on the thread that called tdu_open_*.
 * Strings are NUL-terminated UTF-8.
 */
#ifndef THREE_DU_H
#define THREE_DU_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct TduProject TduProject;
typedef struct TduError TduError;

/* Return false to cancel decoding. done/total count frames (one per count). */
typedef bool (*TduProgressFn)(void *context, uint64_t done, uint64_t total);

enum {
    TDU_OK = 0,
    TDU_ERR_INVALID_ARGUMENT = 1,
    TDU_ERR_IO = 2,
    TDU_ERR_NOT_3DU = 3,
    TDU_ERR_UNSUPPORTED_VERSION = 4,
    TDU_ERR_CORRUPT = 5,
    TDU_ERR_CANCELLED = 6,
    TDU_ERR_REFUSED_LOSSY = 7,
    TDU_ERR_OUTPUT_EXISTS = 8,
    TDU_ERR_VALIDATION = 9,
    TDU_ERR_UNSUPPORTED = 10,
    TDU_ERR_INTERNAL = 99
};

enum {
    TDU_FORMAT_UNKNOWN = 0,
    TDU_FORMAT_3DU = 1,
    TDU_FORMAT_DOTS = 2,
    TDU_FORMAT_FIELDWORK = 3,
    TDU_FORMAT_OM = 4,
    TDU_FORMAT_OMZ = 5,
    TDU_FORMAT_JSON = 6
};

enum {
    TDU_FLAG_STRICT = 1,          /* refuse any warning */
    TDU_FLAG_ALLOW_LOSSY = 2,     /* accept approximations */
    TDU_FLAG_OVERWRITE = 4,       /* replace an existing output */
    TDU_FLAG_PRESERVE_CURVES = 8  /* add derived subsets so curved moves stay exact */
};

const char *tdu_version(void);
int32_t tdu_detect_format(const uint8_t *data, size_t len);

TduProject *tdu_open_path(const char *path, TduProgressFn progress, void *context, TduError **out_error);
TduProject *tdu_open_bytes(const uint8_t *data, size_t len, TduProgressFn progress, void *context, TduError **out_error);

/* JSON summary without names or titles. Free with tdu_string_free. */
char *tdu_project_summary_json(const TduProject *project);
/* Complete intermediate JSON (contains drill data). Free with tdu_string_free. */
char *tdu_project_json(const TduProject *project);

/* Writes atomically after validation. On TDU_OK, *out_diagnostics (if requested) is a JSON
 * array of {severity, code, message}; free it with tdu_string_free. */
int32_t tdu_export(const TduProject *project, int32_t format, const char *output_path, uint32_t flags,
                   char **out_diagnostics, TduError **out_error);

int32_t tdu_error_code(const TduError *error);
const char *tdu_error_message(const TduError *error);

void tdu_error_free(TduError *error);
void tdu_project_free(TduProject *project);
void tdu_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* THREE_DU_H */
