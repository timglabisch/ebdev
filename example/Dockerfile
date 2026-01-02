FROM alpine:latest

RUN adduser -D -s /bin/sh sync && \
    mkdir -p /var/www/frontend /var/www/backend /var/www/shared && \
    chown -R sync:sync /var/www

CMD ["tail", "-f", "/dev/null"]
