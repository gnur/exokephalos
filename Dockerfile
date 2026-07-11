# syntax=docker/dockerfile:1

FROM node:24-alpine AS assets
WORKDIR /src

RUN apk add --no-cache curl

COPY package.json package-lock.json ./
RUN npm ci

COPY static ./static
RUN npm run build:css
RUN mkdir -p static/js \
	&& curl -sL "https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js" -o static/js/htmx.min.js \
	&& curl -sL "https://cdn.jsdelivr.net/npm/chart.js@4/dist/chart.umd.min.js" -o static/js/chart.min.js

FROM golang:1.26.3-alpine AS builder
WORKDIR /src

RUN apk add --no-cache ca-certificates git

COPY go.mod go.sum ./
RUN go mod download

COPY . .
COPY --from=assets /src/static/css/output.css ./static/css/output.css
COPY --from=assets /src/static/js ./static/js

ARG TARGETOS
ARG TARGETARCH
ARG VERSION=dev
ARG BUILD_TIME

RUN CGO_ENABLED=0 GOOS=${TARGETOS:-linux} GOARCH=${TARGETARCH:-amd64} \
	go build \
	-ldflags="-s -w -X github.com/gnur/exokephalos/internal/version.Version=${VERSION} -X github.com/gnur/exokephalos/internal/version.BuildTime=${BUILD_TIME}" \
	-o /out/exo .

FROM gcr.io/distroless/static-debian12:nonroot

COPY --from=builder /out/exo /usr/local/bin/exo

ENV EXO_DIR=/data
VOLUME ["/data"]
EXPOSE 8293

ENTRYPOINT ["/usr/local/bin/exo"]
CMD ["serve"]
