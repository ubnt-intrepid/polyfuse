#define FUSE_USE_VERSION 30

#include <fuse_lowlevel.h>
#include <stddef.h>

struct fuse_session*
fuse_session_new_empty(int argc, char* argv[])
{
    struct fuse_lowlevel_ops op;
    struct fuse_args args = FUSE_ARGS_INIT(argc, argv);
    struct fuse_session* se = NULL;

    se = fuse_session_new(&args, &op, sizeof(struct fuse_lowlevel_ops), NULL);
    fuse_opt_free_args(&args);

    return se;
}
