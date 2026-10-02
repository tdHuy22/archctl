#include "checksum.h"
#include <string.h>

#ifdef __linux__
#include <endian.h>
#else
#include <machine/endian.h>
#endif

int checksum(const unsigned char *buf, unsigned long len) {
    return (int)(len > 0 ? buf[0] : 0);
}
