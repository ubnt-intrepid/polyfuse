#include <fuse_lowlevel.h>
#include <string.h>

struct fuse_session*
fuse_session_new_empty(int argc, char const* const* argv)
{
    struct fuse_args args = FUSE_ARGS_INIT(argc, (char**)argv);
    struct fuse_lowlevel_ops op;

    memset(&op, 0, sizeof(struct fuse_lowlevel_ops));

    return fuse_session_new(&args, &op, sizeof(struct fuse_lowlevel_ops), NULL);
}