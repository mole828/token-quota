# tq-server 是静态链接的 musl 二进制，不需要基础系统。
FROM scratch
COPY tq-server /tq-server
ENV TQ_LISTEN=0.0.0.0:8787 TQ_DATA=/data/state.json
EXPOSE 8787
ENTRYPOINT ["/tq-server"]
