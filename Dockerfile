# The native binary is built by CI (sbt GraalVMNativeImage/packageBin)
# and passed into this image via a build artifact.
FROM gcr.io/distroless/base-debian12
WORKDIR /app

COPY target/native-image/flashcards flashcards

EXPOSE 8080

ENTRYPOINT ["/app/flashcards"]
