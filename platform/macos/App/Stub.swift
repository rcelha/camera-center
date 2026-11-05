// Placeholder executable so Xcode can manage signing and provisioning for the
// app target. The "Embed Rust binary" build phase replaces it with the
// cargo-built `camera-center` binary before code signing.
@main
enum Stub {
    static func main() {}
}
