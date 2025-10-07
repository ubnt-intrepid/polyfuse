
The current target of FUSE ABI is 7.41 (in the kernel v6.12.50).

To upgrade the header file for testing:

```
curl -L "https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/include/uapi/linux/fuse.h?h=v6.16.10" -o ./include/fuse_kernel.h
```


## Kernel ABI versions

* v2.6.14 (ABI 7.2)
  - the header file was located at `include/linux/fuse.h`
* v2.6.15 (ABI 7.3)
* v2.6.16 (ABI 7.6)
* v2.6.18 (ABI 7.7)
* v2.6.20 (ABI 7.8)
* v2.6.24 (ABI 7.9)
* v2.6.28 (ABI 7.10)
* v2.6.29 (ABI 7.11)
* v2.6.31 (ABI 7.12)
* v2.6.32 (ABI 7.13)
* v2.6.35 (ABI 7.14)
* v2.6.36 (ABI 7.15)
* v2.6.38 (ABI 7.16)
* v3.1 (ABI 7.17)
* v3.3 (ABI 7.18)
* v3.5 (ABI 7.19)
* v3.7 (ABI 7.20)
  - the header file was moved to `include/uapi/linux/fuse.h`
* v3.9 (ABI 7.21)
* v3.10 (ABI 7.22)
* v3.15 (ABI 7.23)
* v4.5 (ABI 7.24)
* v4.7 (ABI 7.25)
* v4.9 (ABI 7.26)
* v4.18 (ABI 7.27)
* v4.20 (ABI 7.28)
* v5.1 (ABI 7.29)
* v5.2 (ABI 7.31)
* v5.10 (ABI 7.32)
* v5.11 (ABI 7.33)
* v5.14 (ABI 7.34)
* v5.16 (ABI 7.35)
* v5.17 (ABI 7.36)
* v6.1 (ABI 7.37)
* v6.2 (ABI 7.38)
* v6.6 (ABI 7.39)
* v6.9 (ABI 7.40)
* v6.12 (ABI 7.41)
* v6.14 (ABI 7.42)
* v6.15 (ABI 7.43)
* v6.16 (ABI 7.44)
