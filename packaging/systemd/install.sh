#!/bin/bash
set -e
# §25 systemd 安装，Driver PR_SET_PDEATHSIG 由 Driver 进程内设置，Core 侧无需额外 Job
install -m 755 target/release/forgelinkd /opt/forgelink/forgelinkd
mkdir -p /opt/forgelink/drivers /var/lib/forgelink
cp -r drivers/* /opt/forgelink/drivers/
cp packaging/systemd/forgelink.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now forgelink
echo "ForgeLink systemd installed, status: systemctl status forgelink"
