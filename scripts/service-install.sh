#!/bin/fish

# Ensure that the group and user `audfwd` exist
if not getent group audfwd
  sudo groupadd --system audfwd
end
if not getent passwd audfwd
  sudo useradd --system --no-create-home --shell=/sbin/nologin -g audfwd -G audio audfwd
end

id audfwd

# Change the ownership of the files
sudo chown root:root audio-forwarder.service

# Move files into place
sudo mv audio-forwarder.service "/etc/systemd/system/audio-forwarder.service"

# Enable the service but don't start it
sudo systemctl enable "audio-forwarder.service"