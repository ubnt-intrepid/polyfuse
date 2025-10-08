// compatible types for old kernel ABI.

#ifndef _FUSE_KERNEL_COMPAT_H
#define _FUSE_KERNEL_COMPAT_H

#include <stdint.h>

// ... from kernel v2.6.20

struct fuse_kstatfs_compat_3 {
	uint64_t	blocks;
	uint64_t	bfree;
	uint64_t	bavail;
	uint64_t	files;
	uint64_t	ffree;
	uint32_t	bsize;
	uint32_t	namelen;
};

struct fuse_statfs_out_compat_3 {
	struct fuse_kstatfs_compat_3 st;
};

struct fuse_init_out_compat_3 {
	uint32_t	major;
	uint32_t	minor;
};

// from kernel 2.6.20

struct fuse_attr_compat_8 {
	uint64_t	ino;
	uint64_t	size;
	uint64_t	blocks;
	uint64_t	atime;
	uint64_t	mtime;
	uint64_t	ctime;
	uint32_t	atimensec;
	uint32_t	mtimensec;
	uint32_t	ctimensec;
	uint32_t	mode;
	uint32_t	nlink;
	uint32_t	uid;
	uint32_t	gid;
	uint32_t	rdev;
};

struct fuse_attr_out_compat_8 {
	uint64_t	attr_valid;
	uint32_t	attr_valid_nsec;
	uint32_t	dummy;
	struct fuse_attr_compat_8 attr;
};

struct fuse_entry_out_compat_8 {
	uint64_t	nodeid;
	uint64_t	generation;
	uint64_t	entry_valid;
	uint64_t	attr_valid;
	uint32_t	entry_valid_nsec;
	uint32_t	attr_valid_nsec;
	struct fuse_attr_compat_8 attr;
};

struct fuse_write_in_compat_8 {
	uint64_t	fh;
	uint64_t	offset;
	uint32_t	size;
	uint32_t	write_flags;
};

// from kernel 2.6.29

struct fuse_mknod_in_compat_11 {
	uint32_t	mode;
	uint32_t	rdev;
};

// from kernel v3.10

struct fuse_init_out_compat_22 {
	uint32_t	major;
	uint32_t	minor;
	uint32_t	max_readahead;
	uint32_t	flags;
	uint16_t	max_background;
	uint16_t	congestion_threshold;
	uint32_t	max_write;
};

// from kernel v5.10

struct fuse_setxattr_in_compat_32 {
	uint32_t	size;
	uint32_t	flags;
};

// from kernel 5.16

struct fuse_init_in_compat_35 {
	uint32_t	major;
	uint32_t	minor;
	uint32_t	max_readahead;
	uint32_t	flags;
};

#endif
