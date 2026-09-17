FROM debian:12-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        bash \
        binutils \
        ca-certificates \
        file \
        gcc \
        git \
        libc6-dev \
        libcrypt-dev \
        libssl-dev \
        make \
    && rm -rf /var/lib/apt/lists/*

RUN test "$(getconf GNU_LIBC_VERSION)" = 'glibc 2.36'

ARG USERNAME=vscode
ARG USER_UID=1000
ARG USER_GID=1000

RUN groupadd --gid "${USER_GID}" "${USERNAME}" \
    && useradd --uid "${USER_UID}" --gid "${USER_GID}" --create-home --shell /bin/bash "${USERNAME}"

USER ${USERNAME}
ENV HOME=/home/${USERNAME}
CMD ["bash"]
