// compatible types for old kernel ABI.

#ifndef _FUSE_KERNEL_COMPAT_H
#define _FUSE_KERNEL_COMPAT_H

#include <stdint.h>

struct fuse_setxattr_in_compat_32 {
	uint32_t	size;
	uint32_t	flags;
};

struct fuse_init_in_compat_35 {
	uint32_t	major;
	uint32_t	minor;
	uint32_t	max_readahead;
	uint32_t	flags;
};

#endif
