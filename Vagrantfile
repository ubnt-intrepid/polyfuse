# -*- mode: ruby -*-
# vi: set ft=ruby :

Vagrant.configure("2") do |config|
  config.vm.define "fedora", autostart: false do |config|
    config.vm.box = "fedora/31-cloud-base"
    config.vm.provision "shell", privileged: false, inline: <<-SHELL
      sudo dnf install -y gcc pkg-config fuse fuse-devel
      curl -sSf https://sh.rustup.rs | sh -s -- -y --profile=minimal
    SHELL
  end

  config.vm.define "freebsd", autostart: false do |config|
    config.vm.box = "generic/freebsd12"
    config.ssh.shell = "sh"
    config.vm.provision "shell", privileged: false, inline: <<-SHELL
      sudo pkg install -y pkgconf fusefs-libs
      fetch https://sh.rustup.rs -o rustup.sh
     sh rustup.sh -y --profile=minimal
    SHELL
  end

  config.vm.provider :libvirt do |libvirt|
    libvirt.cpus = 2
    libvirt.memory = 2048
  end

  config.vm.synced_folder ".", "/vagrant", type: "rsync",
    rsync__auto: false,
    rsync__exclude: [ ".git/", "target/" ]
end
