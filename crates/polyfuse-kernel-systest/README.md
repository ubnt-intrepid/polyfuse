
The current target of FUSE ABI is 7.41 (in the kernel v6.12.50).

To upgrade the header file for testing:

```
curl -L "https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/include/uapi/linux/fuse.h?h=v6.16.10" -o ./include/fuse_kernel.h
```
