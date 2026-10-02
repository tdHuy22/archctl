#include "parser.h"
#include "checksum.h"
#include <stdio.h>

/* Vi phạm: `deny * -> LibPcap except Capture`. Chỉ Capture được chạm libpcap. */
#include <pcap.h>

/* Ba dòng dưới KHÔNG phải include. Nếu lexer tính chúng thì sinh vi phạm giả. */
/* #include <pcap.h> */
// #include <pcap.h>
static const char *doc = "#include <pcap.h>";

int parse_ethernet(const unsigned char *buf, unsigned long len) {
    (void)doc;
    (void)pcap_lib_version();
    return checksum(buf, len) ? 0 : fprintf(stderr, "bad\n");
}
