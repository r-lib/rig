#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

const char *rig_last_error(void);

int rig_get_default(char *ptr, size_t size);

int rig_list(char *ptr, size_t size);

int rig_list_with_versions(char *ptr, size_t size);

int rig_set_default(const char *ptr);

int rig_start_rstudio(const char *pversion, const char *pproject);
