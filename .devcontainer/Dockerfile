FROM mcr.microsoft.com/devcontainers/base:bookworm

# Install devbox (https://github.com/jetify-com/devbox)
# Uses curl-pipe pattern — standard for devcontainer setup.
# To pin a version: replace URL with a tagged release installer.
RUN curl -fsSL https://get.jetify.com/devbox | bash -s -- -f

USER vscode
