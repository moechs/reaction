# This Dockerfile permits to build reaction and its plugins

# Use debian old-stable, so that it runs on both old-stable and stable
FROM rust:bookworm

RUN apt update && apt install -y \
  clang \
  libipset-dev \
  libnftables-dev \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /reaction
