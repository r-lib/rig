#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Bumped whenever the layout changes. It is part of the file *name*, so an
 * old blob is never read at all rather than being read and rejected.
 */
#define FORMAT_VERSION 1

int rig_last_error(char *ptr, size_t size);

int rig_mode(char *ptr, size_t size);

int rig_r_root(char *ptr, size_t size);

int rig_get_default(char *ptr, size_t size);

int rig_list(char *ptr, size_t size);

int rig_list_with_versions(char *ptr, size_t size);

int rig_set_default(const char *ptr);

int rig_start_rstudio(const char *pversion, const char *pproject);

int rig_library_list(char *ptr, size_t size);

int rig_lib_set_default(const char *ptr);
