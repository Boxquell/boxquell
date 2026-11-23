# Root filesystem for boxquell guests. Exported and packed into a raw ext4 image by build.sh.
FROM debian:stable-slim
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      ca-certificates curl git iproute2 iputils-ping less openssh-server e2fsprogs procps sudo \
      initramfs-tools zstd \
 && printf '%s\n' virtio_mmio virtio_pci virtio_blk virtio_net virtio_console ext4 virtiofs 9p 9pnet_virtio \
      >> /etc/initramfs-tools/modules \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends linux-image-cloud-amd64 \
 && rm -rf /var/lib/apt/lists/*
# The kernel and initrd are only used by the QEMU backend; libkrun brings its own kernel.
RUN useradd -m -s /bin/bash dev \
 && echo 'dev ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/dev \
 && mkdir -p /workspace && chown dev:dev /workspace \
 && cp /etc/ssl/certs/ca-certificates.crt /etc/ssl/boxquell-base-ca.crt \
 && printf '%s\n' 'PasswordAuthentication no' 'PermitRootLogin no' > /etc/ssh/sshd_config.d/boxquell.conf
COPY boxquell-init boxquell-shutdown /usr/local/sbin/
