FROM ubuntu:24.04
ENV container=docker
RUN apt-get update && apt-get install -y --no-install-recommends systemd systemd-sysv python3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY . /opt/pir-updater/
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
