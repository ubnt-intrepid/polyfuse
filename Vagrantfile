# -*- mode: ruby -*-
# vi: set ft=ruby :

Vagrant.configure("2") do |config|
  config.vm.box = "generic/freebsd12"
  config.vm.provider "libvirt"
  config.vm.provider "virtualbox"

  config.ssh.shell = "sh"

  # config.vm.network "forwarded_port", guest: 80, host: 8080
  # config.vm.network "forwarded_port", guest: 80, host: 8080, host_ip: "127.0.0.1"
  # config.vm.network "private_network", ip: "192.168.33.10"
  # config.vm.network "public_network"

  config.vm.synced_folder ".", "/vagrant", type: "rsync",
    rsync__exclude: [".git/", "target/" ]

  config.vm.provision "shell", privileged: false, inline: <<-SHELL
    sudo pkg install -y pkgconf fusefs-libs
    fetch https://sh.rustup.rs -o rustup.sh
    sh rustup.sh -y
  SHELL
end
