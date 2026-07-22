# syntax=docker/dockerfile:1

FROM node:24-alpine AS assets
WORKDIR /src

COPY package.json package-lock.json ./
RUN npm ci

COPY static ./static
COPY templates ./templates
COPY web ./web
COPY vite.config.ts ./
RUN npm run build:css
RUN npm run build:web

FROM golang:1.26.3-alpine AS builder
WORKDIR /src

RUN apk add --no-cache ca-certificates git

COPY go.mod go.sum ./
RUN go mod download

COPY . .
COPY --from=assets /src/static/css/output.css ./static/css/output.css
COPY --from=assets /src/web/dist ./web/dist

ARG TARGETOS
ARG TARGETARCH
ARG VERSION=dev
ARG BUILD_TIME

RUN CGO_ENABLED=0 GOOS=${TARGETOS:-linux} GOARCH=${TARGETARCH:-amd64} \
	go build \
	-ldflags="-s -w -X github.com/gnur/exokephalos/internal/version.Version=${VERSION} -X github.com/gnur/exokephalos/internal/version.BuildTime=${BUILD_TIME}" \
	-o /out/xo .

FROM gcr.io/distroless/static-debian12:nonroot

COPY --from=builder /out/xo /usr/local/bin/xo

ENV EXO_DIR=/data
VOLUME ["/data"]
EXPOSE 8293

ENTRYPOINT ["/usr/local/bin/xo"]
CMD ["serve"]
