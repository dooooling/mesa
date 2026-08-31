#!/bin/bash
set -e
# §25 systemd 安装，Driver PR_SET_PDEATHSIG 由 Driver 进程内设置，Core 侧无需额外 Job
install -m 755 target/release/mesad /opt/mesa/mesad
mkdir -p /opt/mesa/drivers /var/lib/mesa
cp -r drivers/* /opt/mesa/drivers/
cp packaging/systemd/mesa.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now mesa
echo "Mesa systemd installed, status: systemctl status mesa"
