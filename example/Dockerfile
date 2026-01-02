FROM debian:bookworm-slim

RUN useradd -m -s /bin/bash ebdev && \
    mkdir -p /var/www/frontend /var/www/backend /var/www/shared && \
    chown -R ebdev:ebdev /var/www

CMD ["tail", "-f", "/dev/null"]
