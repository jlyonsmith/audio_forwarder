#!/bin/fish

set service_active (systemctl is-active --quiet "audio-forwarder.service"; echo $status)

# Stop service if it is running
if test $service_active -eq 0
    sudo systemctl stop "audio-forwarder.service"
end

# Change the ownership of the files
sudo chown root:root "audio-forwarder"

# Move files into place
sudo mv "audio-forwarder" "/usr/local/bin/audio-forwarder"

# Restart the service if it was running
if test $service_active -eq 0
    sudo systemctl start "audio-forwarder.service"
end
