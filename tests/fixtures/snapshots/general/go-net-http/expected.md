### Overview

- [Clients and Transports](#hdr-Clients_and_Transports)
- [Servers](#hdr-Servers)
- [HTTP/2](#hdr-HTTP_2)

Package http provides HTTP client and server implementations.

[Get](#Get), [Head](#Head), [Post](#Post), and [PostForm](#PostForm) make HTTP (or HTTPS) requests:

```
resp, err := http.Get("http://example.com/")
...
resp, err := http.Post("http://example.com/upload", "image/jpeg", &buf)
...
resp, err := http.PostForm("http://example.com/form",
	url.Values{"key": {"Value"}, "id": {"123"}})
```

The caller must close the response body when finished with it:

```
resp, err := http.Get("http://example.com/")
if err != nil {
	// handle error
}
defer resp.Body.Close()
body, err := io.ReadAll(resp.Body)
// ...
```

#### Clients and Transports

For control over HTTP client headers, redirect policy, and other settings, create a [Client](#Client):

```
client := &http.Client{
	CheckRedirect: redirectPolicyFunc,
}

resp, err := client.Get("http://example.com")
// ...

req, err := http.NewRequest("GET", "http://example.com", nil)
// ...
req.Header.Add("If-None-Match", `W/"wyzzy"`)
resp, err := client.Do(req)
// ...
```

For control over proxies, TLS configuration, keep-alives, compression, and other settings, create a [Transport](#Transport):

```
tr := &http.Transport{
	MaxIdleConns:       10,
	IdleConnTimeout:    30 * time.Second,
	DisableCompression: true,
}
client := &http.Client{Transport: tr}
resp, err := client.Get("https://example.com")
```

Clients and Transports are safe for concurrent use by multiple goroutines and for efficiency should only be created once and re-used.

#### Servers

ListenAndServe starts an HTTP server with a given address and handler. The handler is usually nil, which means to use [DefaultServeMux](#DefaultServeMux). [Handle](#Handle) and [HandleFunc](#HandleFunc) add handlers to [DefaultServeMux](#DefaultServeMux):

```
http.Handle("/foo", fooHandler)

http.HandleFunc("/bar", func(w http.ResponseWriter, r *http.Request) {
	fmt.Fprintf(w, "Hello, %q", html.EscapeString(r.URL.Path))
})

log.Fatal(http.ListenAndServe(":8080", nil))
```

More control over the server's behavior is available by creating a custom Server:

```
s := &http.Server{
	Addr:           ":8080",
	Handler:        myHandler,
	ReadTimeout:    10 * time.Second,
	WriteTimeout:   10 * time.Second,
	MaxHeaderBytes: 1 << 20,
}
log.Fatal(s.ListenAndServe())
```

#### HTTP/2

The http package has transparent support for the HTTP/2 protocol.

[Server](#Server) and [DefaultTransport](#DefaultTransport) automatically enable HTTP/2 support when using HTTPS. [Transport](#Transport) does not enable HTTP/2 by default.

To enable or disable support for HTTP/1, HTTP/2, and/or unencrypted HTTP/2, see the [Server.Protocols](#Server.Protocols) and [Transport.Protocols](#Transport.Protocols) configuration fields.

To configure advanced HTTP/2 features, see the [Server.HTTP2](#Server.HTTP2) and [Transport.HTTP2](#Transport.HTTP2) configuration fields.

Alternatively, the following GODEBUG settings are currently supported:

```
GODEBUG=http2client=0  # disable HTTP/2 client support
GODEBUG=http2server=0  # disable HTTP/2 server support
GODEBUG=http2debug=1   # enable verbose HTTP/2 debug logs
GODEBUG=http2debug=2   # ... even more verbose, with frame dumps
```

The "omithttp2" build tag may be used to disable the HTTP/2 implementation contained in the http package.

### Index

- [Constants](#pkg-constants)
- [Variables](#pkg-variables)
- [func CanonicalHeaderKey(s string) string](#CanonicalHeaderKey)
- [func DetectContentType(data \[\]byte) string](#DetectContentType)
- [func Error(w ResponseWriter, error string, code int)](#Error)
- [func Handle(pattern string, handler Handler)](#Handle)
- [func HandleFunc(pattern string, handler func(ResponseWriter, \*Request))](#HandleFunc)
- [func ListenAndServe(addr string, handler Handler) error](#ListenAndServe)
- [func ListenAndServeTLS(addr, certFile, keyFile string, handler Handler) error](#ListenAndServeTLS)
- [func MaxBytesReader(w ResponseWriter, r io.ReadCloser, n int64) io.ReadCloser](#MaxBytesReader)
- [func NotFound(w ResponseWriter, r \*Request)](#NotFound)
- [func ParseHTTPVersion(vers string) (major, minor int, ok bool)](#ParseHTTPVersion)
- [func ParseTime(text string) (t time.Time, err error)](#ParseTime)
- [func ProxyFromEnvironment(req \*Request) (\*url.URL, error)](#ProxyFromEnvironment)
- [func ProxyURL(fixedURL \*url.URL) func(\*Request) (\*url.URL, error)](#ProxyURL)
- [func Redirect(w ResponseWriter, r \*Request, url string, code int)](#Redirect)
- [func Serve(l net.Listener, handler Handler) error](#Serve)
- [func ServeContent(w ResponseWriter, req \*Request, name string, modtime time.Time, ...)](#ServeContent)
- [func ServeFile(w ResponseWriter, r \*Request, name string)](#ServeFile)
- [func ServeFileFS(w ResponseWriter, r \*Request, fsys fs.FS, name string)](#ServeFileFS)
- [func ServeTLS(l net.Listener, handler Handler, certFile, keyFile string) error](#ServeTLS)
- [func SetCookie(w ResponseWriter, cookie \*Cookie)](#SetCookie)
- [func StatusText(code int) string](#StatusText)
- [type Client](#Client)
- 
  - [func (c \*Client) CloseIdleConnections()](#Client.CloseIdleConnections)
  - [func (c \*Client) Do(req \*Request) (\*Response, error)](#Client.Do)
  - [func (c \*Client) Get(url string) (resp \*Response, err error)](#Client.Get)
  - [func (c \*Client) Head(url string) (resp \*Response, err error)](#Client.Head)
  - [func (c \*Client) Post(url, contentType string, body io.Reader) (resp \*Response, err error)](#Client.Post)
  - [func (c \*Client) PostForm(url string, data url.Values) (resp \*Response, err error)](#Client.PostForm)
- [type ClientConn](#ClientConn)
- 
  - [func (cc \*ClientConn) Available() int](#ClientConn.Available)
  - [func (cc \*ClientConn) Close() error](#ClientConn.Close)
  - [func (cc \*ClientConn) Err() error](#ClientConn.Err)
  - [func (cc \*ClientConn) InFlight() int](#ClientConn.InFlight)
  - [func (cc \*ClientConn) Release()](#ClientConn.Release)
  - [func (cc \*ClientConn) Reserve() error](#ClientConn.Reserve)
  - [func (cc \*ClientConn) RoundTrip(req \*Request) (\*Response, error)](#ClientConn.RoundTrip)
  - [func (cc \*ClientConn) SetStateHook(f func(\*ClientConn))](#ClientConn.SetStateHook)
- [type CloseNotifier](#CloseNotifier) deprecated
- [type ConnState](#ConnState)
- 
  - [func (c ConnState) String() string](#ConnState.String)
- [type Cookie](#Cookie)
- 
  - [func ParseCookie(line string) (\[\]\*Cookie, error)](#ParseCookie)
  - [func ParseSetCookie(line string) (\*Cookie, error)](#ParseSetCookie)
- 
  - [func (c \*Cookie) String() string](#Cookie.String)
  - [func (c \*Cookie) Valid() error](#Cookie.Valid)
- [type CookieJar](#CookieJar)
- [type CrossOriginProtection](#CrossOriginProtection)
- 
  - [func NewCrossOriginProtection() \*CrossOriginProtection](#NewCrossOriginProtection)
- 
  - [func (c \*CrossOriginProtection) AddInsecureBypassPattern(pattern string)](#CrossOriginProtection.AddInsecureBypassPattern)
  - [func (c \*CrossOriginProtection) AddTrustedOrigin(origin string) error](#CrossOriginProtection.AddTrustedOrigin)
  - [func (c \*CrossOriginProtection) Check(req \*Request) error](#CrossOriginProtection.Check)
  - [func (c \*CrossOriginProtection) Handler(h Handler) Handler](#CrossOriginProtection.Handler)
  - [func (c \*CrossOriginProtection) SetDenyHandler(h Handler)](#CrossOriginProtection.SetDenyHandler)
- [type Dir](#Dir)
- 
  - [func (d Dir) Open(name string) (File, error)](#Dir.Open)
- [type File](#File)
- [type FileSystem](#FileSystem)
- 
  - [func FS(fsys fs.FS) FileSystem](#FS)
- [type Flusher](#Flusher)
- [type HTTP2Config](#HTTP2Config)
- [type Handler](#Handler)
- 
  - [func AllowQuerySemicolons(h Handler) Handler](#AllowQuerySemicolons)
  - [func FileServer(root FileSystem) Handler](#FileServer)
  - [func FileServerFS(root fs.FS) Handler](#FileServerFS)
  - [func MaxBytesHandler(h Handler, n int64) Handler](#MaxBytesHandler)
  - [func NotFoundHandler() Handler](#NotFoundHandler)
  - [func RedirectHandler(url string, code int) Handler](#RedirectHandler)
  - [func StripPrefix(prefix string, h Handler) Handler](#StripPrefix)
  - [func TimeoutHandler(h Handler, dt time.Duration, msg string) Handler](#TimeoutHandler)
- [type HandlerFunc](#HandlerFunc)
- 
  - [func (f HandlerFunc) ServeHTTP(w ResponseWriter, r \*Request)](#HandlerFunc.ServeHTTP)
- [type Header](#Header)
- 
  - [func (h Header) Add(key, value string)](#Header.Add)
  - [func (h Header) Clone() Header](#Header.Clone)
  - [func (h Header) Del(key string)](#Header.Del)
  - [func (h Header) Get(key string) string](#Header.Get)
  - [func (h Header) Set(key, value string)](#Header.Set)
  - [func (h Header) Values(key string) \[\]string](#Header.Values)
  - [func (h Header) Write(w io.Writer) error](#Header.Write)
  - [func (h Header) WriteSubset(w io.Writer, exclude map\[string\]bool) error](#Header.WriteSubset)
- [type Hijacker](#Hijacker)
- [type MaxBytesError](#MaxBytesError)
- 
  - [func (e \*MaxBytesError) Error() string](#MaxBytesError.Error)
- [type ProtocolError](#ProtocolError) deprecated
- 
  - [func (pe \*ProtocolError) Error() string](#ProtocolError.Error)
  - [func (pe \*ProtocolError) Is(err error) bool](#ProtocolError.Is)
- [type Protocols](#Protocols)
- 
  - [func (p Protocols) HTTP1() bool](#Protocols.HTTP1)
  - [func (p Protocols) HTTP2() bool](#Protocols.HTTP2)
  - [func (p \*Protocols) SetHTTP1(ok bool)](#Protocols.SetHTTP1)
  - [func (p \*Protocols) SetHTTP2(ok bool)](#Protocols.SetHTTP2)
  - [func (p \*Protocols) SetUnencryptedHTTP2(ok bool)](#Protocols.SetUnencryptedHTTP2)
  - [func (p Protocols) String() string](#Protocols.String)
  - [func (p Protocols) UnencryptedHTTP2() bool](#Protocols.UnencryptedHTTP2)
- [type PushOptions](#PushOptions)
- [type Pusher](#Pusher)
- [type Request](#Request)
- 
  - [func NewRequest(method, url string, body io.Reader) (\*Request, error)](#NewRequest)
  - [func NewRequestWithContext(ctx context.Context, method, url string, body io.Reader) (\*Request, error)](#NewRequestWithContext)
  - [func ReadRequest(b \*bufio.Reader) (\*Request, error)](#ReadRequest)
- 
  - [func (r \*Request) AddCookie(c \*Cookie)](#Request.AddCookie)
  - [func (r \*Request) BasicAuth() (username, password string, ok bool)](#Request.BasicAuth)
  - [func (r \*Request) Clone(ctx context.Context) \*Request](#Request.Clone)
  - [func (r \*Request) Context() context.Context](#Request.Context)
  - [func (r \*Request) Cookie(name string) (\*Cookie, error)](#Request.Cookie)
  - [func (r \*Request) Cookies() \[\]\*Cookie](#Request.Cookies)
  - [func (r \*Request) CookiesNamed(name string) \[\]\*Cookie](#Request.CookiesNamed)
  - [func (r \*Request) FormFile(key string) (multipart.File, \*multipart.FileHeader, error)](#Request.FormFile)
  - [func (r \*Request) FormValue(key string) string](#Request.FormValue)
  - [func (r \*Request) MultipartReader() (\*multipart.Reader, error)](#Request.MultipartReader)
  - [func (r \*Request) ParseForm() error](#Request.ParseForm)
  - [func (r \*Request) ParseMultipartForm(maxMemory int64) error](#Request.ParseMultipartForm)
  - [func (r \*Request) PathValue(name string) string](#Request.PathValue)
  - [func (r \*Request) PostFormValue(key string) string](#Request.PostFormValue)
  - [func (r \*Request) ProtoAtLeast(major, minor int) bool](#Request.ProtoAtLeast)
  - [func (r \*Request) Referer() string](#Request.Referer)
  - [func (r \*Request) SetBasicAuth(username, password string)](#Request.SetBasicAuth)
  - [func (r \*Request) SetPathValue(name, value string)](#Request.SetPathValue)
  - [func (r \*Request) UserAgent() string](#Request.UserAgent)
  - [func (r \*Request) WithContext(ctx context.Context) \*Request](#Request.WithContext)
  - [func (r \*Request) Write(w io.Writer) error](#Request.Write)
  - [func (r \*Request) WriteProxy(w io.Writer) error](#Request.WriteProxy)
- [type Response](#Response)
- 
  - [func Get(url string) (resp \*Response, err error)](#Get)
  - [func Head(url string) (resp \*Response, err error)](#Head)
  - [func Post(url, contentType string, body io.Reader) (resp \*Response, err error)](#Post)
  - [func PostForm(url string, data url.Values) (resp \*Response, err error)](#PostForm)
  - [func ReadResponse(r \*bufio.Reader, req \*Request) (\*Response, error)](#ReadResponse)
- 
  - [func (r \*Response) Cookies() \[\]\*Cookie](#Response.Cookies)
  - [func (r \*Response) Location() (\*url.URL, error)](#Response.Location)
  - [func (r \*Response) ProtoAtLeast(major, minor int) bool](#Response.ProtoAtLeast)
  - [func (r \*Response) Write(w io.Writer) error](#Response.Write)
- [type ResponseController](#ResponseController)
- 
  - [func NewResponseController(rw ResponseWriter) \*ResponseController](#NewResponseController)
- 
  - [func (c \*ResponseController) EnableFullDuplex() error](#ResponseController.EnableFullDuplex)
  - [func (c \*ResponseController) Flush() error](#ResponseController.Flush)
  - [func (c \*ResponseController) Hijack() (net.Conn, \*bufio.ReadWriter, error)](#ResponseController.Hijack)
  - [func (c \*ResponseController) SetReadDeadline(deadline time.Time) error](#ResponseController.SetReadDeadline)
  - [func (c \*ResponseController) SetWriteDeadline(deadline time.Time) error](#ResponseController.SetWriteDeadline)
- [type ResponseWriter](#ResponseWriter)
- [type RoundTripper](#RoundTripper)
- 
  - [func NewFileTransport(fs FileSystem) RoundTripper](#NewFileTransport)
  - [func NewFileTransportFS(fsys fs.FS) RoundTripper](#NewFileTransportFS)
- [type SameSite](#SameSite)
- [type ServeMux](#ServeMux)
- 
  - [func NewServeMux() \*ServeMux](#NewServeMux)
- 
  - [func (mux \*ServeMux) Handle(pattern string, handler Handler)](#ServeMux.Handle)
  - [func (mux \*ServeMux) HandleFunc(pattern string, handler func(ResponseWriter, \*Request))](#ServeMux.HandleFunc)
  - [func (mux \*ServeMux) Handler(r \*Request) (h Handler, pattern string)](#ServeMux.Handler)
  - [func (mux \*ServeMux) ServeHTTP(w ResponseWriter, r \*Request)](#ServeMux.ServeHTTP)
- [type Server](#Server)
- 
  - [func (s \*Server) Close() error](#Server.Close)
  - [func (s \*Server) ListenAndServe() error](#Server.ListenAndServe)
  - [func (s \*Server) ListenAndServeTLS(certFile, keyFile string) error](#Server.ListenAndServeTLS)
  - [func (s \*Server) RegisterOnShutdown(f func())](#Server.RegisterOnShutdown)
  - [func (s \*Server) Serve(l net.Listener) error](#Server.Serve)
  - [func (s \*Server) ServeTLS(l net.Listener, certFile, keyFile string) error](#Server.ServeTLS)
  - [func (s \*Server) SetKeepAlivesEnabled(v bool)](#Server.SetKeepAlivesEnabled)
  - [func (s \*Server) Shutdown(ctx context.Context) error](#Server.Shutdown)
- [type Transport](#Transport)
- 
  - [func (t \*Transport) CancelRequest(req \*Request)](#Transport.CancelRequest)deprecated
  - [func (t \*Transport) Clone() \*Transport](#Transport.Clone)
  - [func (t \*Transport) CloseIdleConnections()](#Transport.CloseIdleConnections)
  - [func (t \*Transport) NewClientConn(ctx context.Context, scheme, address string) (\*ClientConn, error)](#Transport.NewClientConn)
  - [func (t \*Transport) RegisterProtocol(scheme string, rt RoundTripper)](#Transport.RegisterProtocol)
  - [func (t \*Transport) RoundTrip(req \*Request) (\*Response, error)](#Transport.RoundTrip)

### Examples

- [CrossOriginProtection](#example-CrossOriginProtection)
- [FileServer](#example-FileServer)
- [FileServer (DotFileHiding)](#example-FileServer-DotFileHiding)
- [FileServer (StripPrefix)](#example-FileServer-StripPrefix)
- [Get](#example-Get)
- [Handle](#example-Handle)
- [HandleFunc](#example-HandleFunc)
- [Hijacker](#example-Hijacker)
- [ListenAndServe](#example-ListenAndServe)
- [ListenAndServeTLS](#example-ListenAndServeTLS)
- [NotFoundHandler](#example-NotFoundHandler)
- [Protocols (Http1)](#example-Protocols-Http1)
- [Protocols (Http1or2)](#example-Protocols-Http1or2)
- [ResponseWriter (Trailers)](#example-ResponseWriter-Trailers)
- [ServeMux.Handle](#example-ServeMux.Handle)
- [Server.Shutdown](#example-Server.Shutdown)
- [StripPrefix](#example-StripPrefix)

### Constants

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/method.go;l=10)

```
const (
	MethodGet     = "GET"
	MethodHead    = "HEAD"
	MethodPost    = "POST"
	MethodPut     = "PUT"
	MethodPatch   = "PATCH" // RFC 5789
	MethodDelete  = "DELETE"
	MethodConnect = "CONNECT"
	MethodOptions = "OPTIONS"
	MethodTrace   = "TRACE"
)
```

Common HTTP methods.

Unless otherwise noted, these are defined in [RFC 7231 section 4.3](https://rfc-editor.org/rfc/rfc7231.html#section-4.3).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/status.go;l=9)

```
const (
	StatusContinue           = 100 // RFC 9110, 15.2.1
	StatusSwitchingProtocols = 101 // RFC 9110, 15.2.2
	StatusProcessing         = 102 // RFC 2518, 10.1
	StatusEarlyHints         = 103 // RFC 8297

	StatusOK                   = 200 // RFC 9110, 15.3.1
	StatusCreated              = 201 // RFC 9110, 15.3.2
	StatusAccepted             = 202 // RFC 9110, 15.3.3
	StatusNoContent            = 204 // RFC 9110, 15.3.5
	StatusResetContent         = 205 // RFC 9110, 15.3.6
	StatusPartialContent       = 206 // RFC 9110, 15.3.7
	StatusMultiStatus          = 207 // RFC 4918, 11.1
	StatusAlreadyReported      = 208 // RFC 5842, 7.1
	StatusIMUsed               = 226 // RFC 3229, 10.4.1

	StatusMultipleChoices  = 300 // RFC 9110, 15.4.1
	StatusMovedPermanently = 301 // RFC 9110, 15.4.2
	StatusFound            = 302 // RFC 9110, 15.4.3
	StatusSeeOther         = 303 // RFC 9110, 15.4.4
	StatusNotModified      = 304 // RFC 9110, 15.4.5
	StatusUseProxy         = 305 // RFC 9110, 15.4.6

	StatusTemporaryRedirect = 307 // RFC 9110, 15.4.8
	StatusPermanentRedirect = 308 // RFC 9110, 15.4.9

	StatusBadRequest                   = 400 // RFC 9110, 15.5.1
	StatusUnauthorized                 = 401 // RFC 9110, 15.5.2
	StatusPaymentRequired              = 402 // RFC 9110, 15.5.3
	StatusForbidden                    = 403 // RFC 9110, 15.5.4
	StatusNotFound                     = 404 // RFC 9110, 15.5.5
	StatusMethodNotAllowed             = 405 // RFC 9110, 15.5.6
	StatusNotAcceptable                = 406 // RFC 9110, 15.5.7
	StatusProxyAuthRequired            = 407 // RFC 9110, 15.5.8
	StatusRequestTimeout               = 408 // RFC 9110, 15.5.9
	StatusConflict                     = 409 // RFC 9110, 15.5.10
	StatusGone                         = 410 // RFC 9110, 15.5.11
	StatusLengthRequired               = 411 // RFC 9110, 15.5.12
	StatusPreconditionFailed           = 412 // RFC 9110, 15.5.13
	StatusRequestEntityTooLarge        = 413 // RFC 9110, 15.5.14
	StatusRequestURITooLong            = 414 // RFC 9110, 15.5.15
	StatusUnsupportedMediaType         = 415 // RFC 9110, 15.5.16
	StatusRequestedRangeNotSatisfiable = 416 // RFC 9110, 15.5.17
	StatusExpectationFailed            = 417 // RFC 9110, 15.5.18
	StatusTeapot                       = 418 // RFC 9110, 15.5.19 (Unused)
	StatusMisdirectedRequest           = 421 // RFC 9110, 15.5.20
	StatusUnprocessableEntity          = 422 // RFC 9110, 15.5.21
	StatusLocked                       = 423 // RFC 4918, 11.3
	StatusFailedDependency             = 424 // RFC 4918, 11.4
	StatusTooEarly                     = 425 // RFC 8470, 5.2.
	StatusUpgradeRequired              = 426 // RFC 9110, 15.5.22
	StatusPreconditionRequired         = 428 // RFC 6585, 3
	StatusTooManyRequests              = 429 // RFC 6585, 4
	StatusRequestHeaderFieldsTooLarge  = 431 // RFC 6585, 5
	StatusUnavailableForLegalReasons   = 451 // RFC 7725, 3

	StatusInternalServerError           = 500 // RFC 9110, 15.6.1
	StatusNotImplemented                = 501 // RFC 9110, 15.6.2
	StatusBadGateway                    = 502 // RFC 9110, 15.6.3
	StatusServiceUnavailable            = 503 // RFC 9110, 15.6.4
	StatusGatewayTimeout                = 504 // RFC 9110, 15.6.5
	StatusHTTPVersionNotSupported       = 505 // RFC 9110, 15.6.6
	StatusVariantAlsoNegotiates         = 506 // RFC 2295, 8.1
	StatusInsufficientStorage           = 507 // RFC 4918, 11.5
	StatusLoopDetected                  = 508 // RFC 5842, 7.2
	StatusNotExtended                   = 510 // RFC 2774, 7
	StatusNetworkAuthenticationRequired = 511 // RFC 6585, 6
)
```

HTTP status codes as registered with IANA. See: [https://www.iana.org/assignments/http-status-codes/http-status-codes.xhtml](https://www.iana.org/assignments/http-status-codes/http-status-codes.xhtml)

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=939)

```
const DefaultMaxHeaderBytes = 1 << 20 // 1 MB
```

DefaultMaxHeaderBytes is the maximum permitted size of the headers in an HTTP request. This can be overridden by setting [Server.MaxHeaderBytes](#Server.MaxHeaderBytes).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=944)

```
const DefaultMaxHeaderValueCount = 500
```

DefaultMaxHeaderValueCount is the maximum permitted number of header values in an HTTP request. This can be overridden by setting [Server.MaxHeaderValueCount](#Server.MaxHeaderValueCount).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=62)

```
const DefaultMaxIdleConnsPerHost = 2
```

DefaultMaxIdleConnsPerHost is the default value of [Transport](#Transport)'s MaxIdleConnsPerHost.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=1027)

```
const TimeFormat = "Mon, 02 Jan 2006 15:04:05 GMT"
```

TimeFormat is the time format to use when generating times in HTTP headers. It is like [time.RFC1123](https://pkg.go.dev/time#RFC1123) but hard-codes GMT as the time zone. The time being formatted must be in UTC for Format to generate the correct format.

For parsing this time format, see [ParseTime](#ParseTime).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=527)

```
const TrailerPrefix = "Trailer:"
```

TrailerPrefix is a magic prefix for [ResponseWriter.Header](#ResponseWriter.Header) map keys that, if present, signals that the map entry is actually for the response trailers, and not the response headers. The prefix is stripped after the ServeHTTP call finishes and the values are sent in the trailers.

This mechanism is intended only for trailers that are not known prior to the headers being written. If the set of trailers is fixed or known before the header is written, the normal Go trailers mechanism is preferred:

```
https://pkg.go.dev/net/http#ResponseWriter
https://pkg.go.dev/net/http#example-ResponseWriter-Trailers
```

### Variables

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=59)

```
var (
	// ErrNotSupported indicates that a feature is not supported.
	//
	// It is returned by ResponseController methods to indicate that
	// the handler does not support the method, and by the Push method
	// of Pusher implementations to indicate that HTTP/2 Push support
	// is not available.
	ErrNotSupported = &ProtocolError{"feature not supported"}

	// Deprecated: ErrUnexpectedTrailer is no longer returned by
	// anything in the net/http package. Callers should not
	// compare errors against this variable.
	ErrUnexpectedTrailer = &ProtocolError{"trailer header without chunked transfer encoding"}

	// ErrMissingBoundary is returned by Request.MultipartReader when the
	// request's Content-Type does not include a "boundary" parameter.
	ErrMissingBoundary = &ProtocolError{"no multipart boundary param in Content-Type"}

	// ErrNotMultipart is returned by Request.MultipartReader when the
	// request's Content-Type is not multipart/form-data.
	ErrNotMultipart = &ProtocolError{"request Content-Type isn't multipart/form-data"}

	// Deprecated: ErrHeaderTooLong is no longer returned by
	// anything in the net/http package. Callers should not
	// compare errors against this variable.
	ErrHeaderTooLong = &ProtocolError{"header too long"}

	// Deprecated: ErrShortBody is no longer returned by
	// anything in the net/http package. Callers should not
	// compare errors against this variable.
	ErrShortBody = &ProtocolError{"entity body too short"}

	// Deprecated: ErrMissingContentLength is no longer returned by
	// anything in the net/http package. Callers should not
	// compare errors against this variable.
	ErrMissingContentLength = &ProtocolError{"missing ContentLength in HEAD response"}
)
```

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=40)

```
var (
	// ErrBodyNotAllowed is returned by ResponseWriter.Write calls
	// when the HTTP method or response code does not permit a
	// body.
	ErrBodyNotAllowed = internal.ErrBodyNotAllowed

	// ErrHijacked is returned by ResponseWriter.Write calls when
	// the underlying connection has been hijacked using the
	// Hijacker interface. A zero-byte write on a hijacked
	// connection will return ErrHijacked without any other side
	// effects.
	ErrHijacked = errors.New("http: connection has been hijacked")

	// ErrContentLength is returned by ResponseWriter.Write calls
	// when a Handler set a Content-Length response header with a
	// declared size and then attempted to write more bytes than
	// declared.
	ErrContentLength = errors.New("http: wrote more than the declared Content-Length")

	// Deprecated: ErrWriteAfterFlush is no longer returned by
	// anything in the net/http package. Callers should not
	// compare errors against this variable.
	ErrWriteAfterFlush = errors.New("unused")
)
```

Errors used by the HTTP server.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=240)

```
var (
	// ServerContextKey is a context key. It can be used in HTTP
	// handlers with Context.Value to access the server that
	// started the handler. The associated value will be of
	// type *Server.
	ServerContextKey = &contextKey{"http-server"}

	// LocalAddrContextKey is a context key. It can be used in
	// HTTP handlers with Context.Value to access the local
	// address the connection arrived on.
	// The associated value will be of type net.Addr.
	LocalAddrContextKey = &contextKey{"local-addr"}
)
```

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=111)

```
var DefaultClient = &Client{}
```

DefaultClient is the default [Client](#Client) and is used by [Get](#Get), [Head](#Head), and [Post](#Post).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2640)

```
var DefaultServeMux = &defaultServeMux
```

DefaultServeMux is the default [ServeMux](#ServeMux) used by [Serve](#Serve).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=1909)

```
var ErrAbortHandler = internal.ErrAbortHandler
```

ErrAbortHandler is a sentinel panic value to abort a handler. While any panic from ServeHTTP aborts the response to the client, panicking with ErrAbortHandler also suppresses logging of a stack trace to the server's error log.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transfer.go;l=838)

```
var ErrBodyReadAfterClose = errors.New("http: invalid Read on closed Body")
```

ErrBodyReadAfterClose is returned when reading a [Request](#Request) or [Response](#Response) Body after the body has been closed. This typically happens when the body is read after an HTTP [Handler](#Handler) calls WriteHeader or Write on its [ResponseWriter](#ResponseWriter).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=4016)

```
var ErrHandlerTimeout = errors.New("http: Handler timeout")
```

ErrHandlerTimeout is returned on [ResponseWriter](#ResponseWriter) Write calls in handlers which have timed out.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transfer.go;l=32)

```
var ErrLineTooLong = internal.ErrLineTooLong
```

ErrLineTooLong is returned when reading request or response bodies with malformed chunked encoding.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=42)

```
var ErrMissingFile = errors.New("http: no such file")
```

ErrMissingFile is returned by FormFile when the provided file field name is either not present in the request or not a file field.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=447)

```
var ErrNoCookie = errors.New("http: named cookie not present")
```

ErrNoCookie is returned by Request's Cookie method when a cookie is not found.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/response.go;l=135)

```
var ErrNoLocation = errors.New("http: no Location header in response")
```

ErrNoLocation is returned by the [Response.Location](#Response.Location) method when no Location header is present.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=214)

```
var ErrSchemeMismatch = errors.New("http: server gave HTTP response to HTTPS client")
```

ErrSchemeMismatch is returned when a server returns an HTTP response to an HTTPS client.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3494)

```
var ErrServerClosed = errors.New("http: Server closed")
```

ErrServerClosed is returned by the [Server.Serve](#Server.Serve), [ServeTLS](#ServeTLS), [ListenAndServe](#ListenAndServe), and [ListenAndServeTLS](#ListenAndServeTLS) methods after a call to [Server.Shutdown](#Server.Shutdown) or [Server.Close](#Server.Close).

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=892)

```
var ErrSkipAltProtocol = internal.ErrSkipAltProtocol
```

ErrSkipAltProtocol is a sentinel error value defined by Transport.RegisterProtocol.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=500)

```
var ErrUseLastResponse = errors.New("net/http: use last response")
```

ErrUseLastResponse can be returned by Client.CheckRedirect hooks to control how redirects are processed. If returned, the next request is not sent and the most recent response is returned with its body unclosed.

[View Source](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=190)

```
var NoBody = noBody{}
```

NoBody is an [io.ReadCloser](https://pkg.go.dev/io#ReadCloser) with no bytes. Read always returns EOF and Close always returns nil. It can be used in an outgoing client request to explicitly signal that a request has zero bytes. An alternative, however, is to simply set [Request.Body](#Request.Body) to nil.

### Functions

#### func [CanonicalHeaderKey](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=234)

```
func CanonicalHeaderKey(s string) string
```

CanonicalHeaderKey returns the canonical format of the header key s. The canonicalization converts the first letter and any letter following a hyphen to upper case; the rest are converted to lowercase. For example, the canonical key for "accept-encoding" is "Accept-Encoding". If s contains a space or invalid header field bytes, it is returned without modifications.

#### func [DetectContentType](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/sniff.go;l=15)

```
func DetectContentType(data []byte) string
```

DetectContentType implements the algorithm described at [https://mimesniff.spec.whatwg.org/](https://mimesniff.spec.whatwg.org/) to determine the Content-Type of the given data. It considers at most the first 512 bytes of data. DetectContentType always returns a valid MIME type: if it cannot determine a more specific one, it returns "application/octet-stream".

#### func [Error](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2353)

```
func Error(w ResponseWriter, error string, code int)
```

Error replies to the request with the specified error message and HTTP code. It does not otherwise end the request; the caller should ensure no further writes are done to w. The error message should be plain text.

Error deletes the Content-Length header, sets Content-Type to “text/plain; charset=utf-8”, and sets X-Content-Type-Options to “nosniff”. This configures the header properly for the error message, in case the caller had set it up expecting a successful output.

#### func [Handle](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2937)

```
func Handle(pattern string, handler Handler)
```

Handle registers the handler for the given pattern in [DefaultServeMux](#DefaultServeMux). The documentation for [ServeMux](#ServeMux) explains how patterns are matched.

Example [¶](#example-Handle "Go to Example")

```
package main

import (
	"fmt"
	"log"
	"net/http"
	"sync"
)

type countHandler struct {
	mu sync.Mutex // guards n
	n  int
}

func (h *countHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.n++
	fmt.Fprintf(w, "count is %d\n", h.n)
}

func main() {
	http.Handle("/count", new(countHandler))
	log.Fatal(http.ListenAndServe(":8080", nil))
}
```



#### func [HandleFunc](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2947)

```
func HandleFunc(pattern string, handler func(ResponseWriter, *Request))
```

HandleFunc registers the handler function for the given pattern in [DefaultServeMux](#DefaultServeMux). The documentation for [ServeMux](#ServeMux) explains how patterns are matched.

Example [¶](#example-HandleFunc "Go to Example")

```
package main

import (
	"io"
	"log"
	"net/http"
)

func main() {
	h1 := func(w http.ResponseWriter, _ *http.Request) {
		io.WriteString(w, "Hello from a HandleFunc #1!\n")
	}
	h2 := func(w http.ResponseWriter, _ *http.Request) {
		io.WriteString(w, "Hello from a HandleFunc #2!\n")
	}

	http.HandleFunc("/", h1)
	http.HandleFunc("/endpoint", h2)

	log.Fatal(http.ListenAndServe(":8080", nil))
}
```



#### func [ListenAndServe](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3811)

```
func ListenAndServe(addr string, handler Handler) error
```

ListenAndServe listens on the TCP network address addr and then calls [Serve](#Serve) with handler to handle requests on incoming connections. Accepted connections are configured to enable TCP keep-alives.

The handler is typically nil, in which case [DefaultServeMux](#DefaultServeMux) is used.

ListenAndServe always returns a non-nil error.

Example [¶](#example-ListenAndServe "Go to Example")

```
package main

import (
	"io"
	"log"
	"net/http"
)

func main() {
	// Hello world, the web server

	helloHandler := func(w http.ResponseWriter, req *http.Request) {
		io.WriteString(w, "Hello, world!\n")
	}

	http.HandleFunc("/hello", helloHandler)
	log.Fatal(http.ListenAndServe(":8080", nil))
}
```



#### func [ListenAndServeTLS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3821)

```
func ListenAndServeTLS(addr, certFile, keyFile string, handler Handler) error
```

ListenAndServeTLS acts identically to [ListenAndServe](#ListenAndServe), except that it expects HTTPS connections. Additionally, files containing a certificate and matching private key for the server must be provided. If the certificate is signed by a certificate authority, the certFile should be the concatenation of the server's certificate, any intermediates, and the CA's certificate.

Example [¶](#example-ListenAndServeTLS "Go to Example")

```
package main

import (
	"io"
	"log"
	"net/http"
)

func main() {
	http.HandleFunc("/", func(w http.ResponseWriter, req *http.Request) {
		io.WriteString(w, "Hello, TLS!\n")
	})

	// One can use generate_cert.go in crypto/tls to generate cert.pem and key.pem.
	log.Printf("About to listen on 8443. Go to https://127.0.0.1:8443/")
	err := http.ListenAndServeTLS(":8443", "cert.pem", "key.pem", nil)
	log.Fatal(err)
}
```



#### func [MaxBytesReader](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1209)

```
func MaxBytesReader(w ResponseWriter, r io.ReadCloser, n int64) io.ReadCloser
```

MaxBytesReader is similar to [io.LimitReader](https://pkg.go.dev/io#LimitReader) but is intended for limiting the size of incoming request bodies. In contrast to io.LimitReader, MaxBytesReader's result is a ReadCloser, returns a non-nil error of type [\*MaxBytesError](#MaxBytesError) for a Read beyond the limit, and closes the underlying reader when its Close method is called.

MaxBytesReader prevents clients from accidentally or maliciously sending a large request and wasting server resources. If possible, it tells the [ResponseWriter](#ResponseWriter) to close the connection after the limit has been reached.

#### func [NotFound](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2374)

```
func NotFound(w ResponseWriter, r *Request)
```

NotFound replies to the request with an HTTP 404 not found error.

#### func [ParseHTTPVersion](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=827)

```
func ParseHTTPVersion(vers string) (major, minor int, ok bool)
```

ParseHTTPVersion parses an HTTP version string according to [RFC 7230, section 2.6](https://rfc-editor.org/rfc/rfc7230.html#section-2.6). "HTTP/1.0" returns (1, 0, true). Note that strings without a minor version, such as "HTTP/2", are not valid.

#### func [ParseTime](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=129) added in go1.1

```
func ParseTime(text string) (t time.Time, err error)
```

ParseTime parses a time header (such as the Date: header), trying each of the three formats allowed by HTTP/1.1: [TimeFormat](#TimeFormat), [time.RFC850](https://pkg.go.dev/time#RFC850), and [time.ANSIC](https://pkg.go.dev/time#ANSIC).

#### func [ProxyFromEnvironment](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=519)

```
func ProxyFromEnvironment(req *Request) (*url.URL, error)
```

ProxyFromEnvironment returns the URL of the proxy to use for a given request, as indicated by the environment variables HTTP\_PROXY, HTTPS\_PROXY and NO\_PROXY (or the lowercase versions thereof). Requests use the proxy from the environment variable matching their scheme, unless excluded by NO\_PROXY.

The environment values may be either a complete URL or a "host\[:port\]", in which case the "http" scheme is assumed. An error is returned if the value is a different form.

A nil URL and nil error are returned if no proxy is defined in the environment, or a proxy should not be used for the given request, as defined by NO\_PROXY.

As a special case, if req.URL.Host is "localhost" (with or without a port number), then a nil URL and nil error will be returned.

#### func [ProxyURL](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=525)

```
func ProxyURL(fixedURL *url.URL) func(*Request) (*url.URL, error)
```

ProxyURL returns a proxy function (for use in a [Transport](#Transport)) that always returns the same URL.

#### func [Redirect](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2419)

```
func Redirect(w ResponseWriter, r *Request, url string, code int)
```

Redirect replies to the request with a redirect to url, which may be a path relative to the request path. Any non-ASCII characters in url will be percent-encoded, but existing percent encodings will not be changed.

The provided code should be in the 3xx range and is usually [StatusMovedPermanently](#StatusMovedPermanently), [StatusFound](#StatusFound) or [StatusSeeOther](#StatusSeeOther).

If the Content-Type header has not been set, [Redirect](#Redirect) sets it to "text/html; charset=utf-8" and writes a small HTML body. Setting the Content-Type header to any value, including nil, disables that behavior.

#### func [Serve](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3016)

```
func Serve(l net.Listener, handler Handler) error
```

Serve accepts incoming HTTP connections on the listener l, creating a new service goroutine for each. The service goroutines read requests and then call handler to reply to them.

The handler is typically nil, in which case [DefaultServeMux](#DefaultServeMux) is used.

HTTP/2 support is only enabled if the Listener returns [\*tls.Conn](https://pkg.go.dev/crypto/tls#Conn) connections or connections which implement the same ConnectionState method as \*tls.Conn, and the connection state indicates that the "h2" protocol was negotiated by ALPN.

Serve always returns a non-nil error.

#### func [ServeContent](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=246)

```
func ServeContent(w ResponseWriter, req *Request, name string, modtime time.Time, content io.ReadSeeker)
```

ServeContent replies to the request using the content in the provided ReadSeeker. The main benefit of ServeContent over [io.Copy](https://pkg.go.dev/io#Copy) is that it handles Range requests properly, sets the MIME type, and handles If-Match, If-Unmodified-Since, If-None-Match, If-Modified-Since, and If-Range requests.

If the response's Content-Type header is not set, ServeContent first tries to deduce the type from name's file extension and, if that fails, falls back to reading the first block of the content and passing it to [DetectContentType](#DetectContentType). The name is otherwise unused; in particular it can be empty and is never sent in the response.

If modtime is not the zero time or Unix epoch, ServeContent includes it in a Last-Modified header in the response. If the request includes an If-Modified-Since header, ServeContent uses modtime to decide whether the content needs to be sent at all.

The content's Seek method must work: ServeContent uses a seek to the end of the content to determine its size. Note that [\*os.File](https://pkg.go.dev/os#File) implements the [io.ReadSeeker](https://pkg.go.dev/io#ReadSeeker) interface.

If the caller has set w's ETag header formatted per [RFC 7232, section 2.3](https://rfc-editor.org/rfc/rfc7232.html#section-2.3), ServeContent uses it to handle requests using If-Match, If-None-Match, or If-Range.

If an error occurs when serving the request (for example, when handling an invalid range request), ServeContent responds with an error message. By default, ServeContent strips the Cache-Control, Content-Encoding, ETag, and Last-Modified headers from error responses. The GODEBUG setting httpservecontentkeepheaders=1 causes ServeContent to preserve these headers.

#### func [ServeFile](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=821)

```
func ServeFile(w ResponseWriter, r *Request, name string)
```

ServeFile replies to the request with the contents of the named file or directory.

If the provided file or directory name is a relative path, it is interpreted relative to the current directory and may ascend to parent directories. If the provided name is constructed from user input, it should be sanitized before calling [ServeFile](#ServeFile).

As a precaution, ServeFile will reject requests where r.URL.Path contains a ".." path element; this protects against callers who might unsafely use [filepath.Join](https://pkg.go.dev/path/filepath#Join) on r.URL.Path without sanitizing it and then use that filepath.Join result as the name argument.

As another special case, ServeFile redirects any request where r.URL.Path ends in "/index.html" to the same path, without the final "index.html". To avoid such redirects either modify the path or use [ServeContent](#ServeContent).

Outside of those two special cases, ServeFile does not use r.URL.Path for selecting the file or directory to serve; only the file or directory provided in the name argument is used.

#### func [ServeFileFS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=855) added in go1.22.0

```
func ServeFileFS(w ResponseWriter, r *Request, fsys fs.FS, name string)
```

ServeFileFS replies to the request with the contents of the named file or directory from the file system fsys. The files provided by fsys must implement [io.Seeker](https://pkg.go.dev/io#Seeker).

If the provided name is constructed from user input, it should be sanitized before calling [ServeFileFS](#ServeFileFS).

As a precaution, ServeFileFS will reject requests where r.URL.Path contains a ".." path element; this protects against callers who might unsafely use [filepath.Join](https://pkg.go.dev/path/filepath#Join) on r.URL.Path without sanitizing it and then use that filepath.Join result as the name argument.

As another special case, ServeFileFS redirects any request where r.URL.Path ends in "/index.html" to the same path, without the final "index.html". To avoid such redirects either modify the path or use [ServeContent](#ServeContent).

Outside of those two special cases, ServeFileFS does not use r.URL.Path for selecting the file or directory to serve; only the file or directory provided in the name argument is used.

#### func [ServeTLS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3033) added in go1.9

```
func ServeTLS(l net.Listener, handler Handler, certFile, keyFile string) error
```

ServeTLS accepts incoming HTTPS connections on the listener l, creating a new service goroutine for each. The service goroutines read requests and then call handler to reply to them.

The handler is typically nil, in which case [DefaultServeMux](#DefaultServeMux) is used.

Additionally, files containing a certificate and matching private key for the server must be provided. If the certificate is signed by a certificate authority, the certFile should be the concatenation of the server's certificate, any intermediates, and the CA's certificate.

ServeTLS always returns a non-nil error.

#### func [SetCookie](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/cookie.go;l=250)

```
func SetCookie(w ResponseWriter, cookie *Cookie)
```

SetCookie adds a Set-Cookie header to the provided [ResponseWriter](#ResponseWriter)'s headers. The provided cookie must have a valid Name. Invalid cookies may be silently dropped.

#### func [StatusText](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/status.go;l=81)

```
func StatusText(code int) string
```

StatusText returns a text for the HTTP status code. It returns the empty string if the code is unknown.


### Types

#### type [Client](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=59)

```
type Client struct {
	// Transport specifies the mechanism by which individual
	// HTTP requests are made.
	// If nil, DefaultTransport is used.
	Transport RoundTripper

	// CheckRedirect specifies the policy for handling redirects.
	// If CheckRedirect is not nil, the client calls it before
	// following an HTTP redirect. The arguments req and via are
	// the upcoming request and the requests made already, oldest
	// first. If CheckRedirect returns an error, the Client's Get
	// method returns both the previous Response (with its Body
	// closed) and CheckRedirect's error (wrapped in a url.Error)
	// instead of issuing the Request req.
	// As a special case, if CheckRedirect returns ErrUseLastResponse,
	// then the most recent response is returned with its body
	// unclosed, along with a nil error.
	//
	// If CheckRedirect is nil, the Client uses its default policy,
	// which is to stop after 10 consecutive requests.
	CheckRedirect func(req *Request, via []*Request) error

	// Jar specifies the cookie jar.
	//
	// The Jar is used to insert relevant cookies into every
	// outbound Request and is updated with the cookie values
	// of every inbound Response. The Jar is consulted for every
	// redirect that the Client follows.
	//
	// If Jar is nil, cookies are only sent if they are explicitly
	// set on the Request.
	Jar CookieJar

	// Timeout specifies a time limit for requests made by this
	// Client. The timeout includes connection time, any
	// redirects, and reading the response body. The timer remains
	// running after Get, Head, Post, or Do return and will
	// interrupt reading of the Response.Body.
	//
	// A Timeout of zero means no timeout.
	//
	// The Client cancels requests to the underlying Transport
	// as if the Request's Context ended.
	//
	// For compatibility, the Client will also use the deprecated
	// CancelRequest method on Transport if found. New
	// RoundTripper implementations should use the Request's Context
	// for cancellation instead of implementing CancelRequest.
	Timeout time.Duration
}
```

A Client is an HTTP client. Its zero value ([DefaultClient](#DefaultClient)) is a usable client that uses [DefaultTransport](#DefaultTransport).

The [Client.Transport](#Client.Transport) typically has internal state (cached TCP connections), so Clients should be reused instead of created as needed. Clients are safe for concurrent use by multiple goroutines.

A Client is higher-level than a [RoundTripper](#RoundTripper) (such as [Transport](#Transport)) and additionally handles HTTP details such as cookies and redirects.

When following redirects, the Client will forward all headers set on the initial [Request](#Request) except:

- when forwarding sensitive headers like "Authorization", "WWW-Authenticate", and "Cookie" to untrusted targets. These headers will be ignored when following a redirect to a domain that is not a subdomain match or exact match of the initial domain. For example, a redirect from "foo.com" to either "foo.com" or "sub.foo.com" will forward the sensitive headers, but a redirect to "bar.com" will not.
- when forwarding the "Cookie" header with a non-nil cookie Jar. Since each redirect may mutate the state of the cookie jar, a redirect may possibly alter a cookie set in the initial request. When forwarding the "Cookie" header, any mutated cookies will be omitted, with the expectation that the Jar will insert those mutated cookies with the updated values (assuming the origin matches). If Jar is nil, the initial cookies are forwarded without change.

#### func (\*Client) [CloseIdleConnections](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=978) added in go1.12

```
func (c *Client) CloseIdleConnections()
```

CloseIdleConnections closes any connections on its [Transport](#Transport) which were previously connected from previous requests but are now sitting idle in a "keep-alive" state. It does not interrupt any connections currently in use.

If [Client.Transport](#Client.Transport) does not have a [Client.CloseIdleConnections](#Client.CloseIdleConnections) method then this method does nothing.

#### func (\*Client) [Do](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=603)

```
func (c *Client) Do(req *Request) (*Response, error)
```

Do sends an HTTP request and returns an HTTP response, following policy (such as redirects, cookies, auth) as configured on the client.

An error is returned if caused by client policy (such as CheckRedirect), or failure to speak HTTP (such as a network connectivity problem). A non-2xx status code doesn't cause an error.

If the returned error is nil, the [Response](#Response) will contain a non-nil Body which the user is expected to close. If the Body is not both read to EOF and closed, the [Client](#Client)'s underlying [RoundTripper](#RoundTripper) (typically [Transport](#Transport)) may not be able to re-use a persistent TCP connection to the server for a subsequent "keep-alive" request. Note, however, that [Transport](#Transport) will automatically try to read a [Response](#Response) Body to EOF asynchronously up to a conservative limit when a Body is closed.

The request Body, if non-nil, will be closed by the underlying Transport, even on errors. The Body may be closed asynchronously after Do returns.

On error, any Response can be ignored. A non-nil Response with a non-nil error only occurs when CheckRedirect fails, and even then the returned [Response.Body](#Response.Body) is already closed.

Generally [Get](#Get), [Post](#Post), or [PostForm](#PostForm) will be used instead of Do.

If the server replies with a redirect, the Client first uses the CheckRedirect function to determine whether the redirect should be followed. If permitted, a 301, 302, or 303 redirect causes subsequent requests to use HTTP method GET (or HEAD if the original request was HEAD), with no body. A 307 or 308 redirect preserves the original HTTP method and body, provided that the [Request.GetBody](#Request.GetBody) function is defined. The [NewRequest](#NewRequest) function automatically sets GetBody for common standard library body types.

Note that the [Client](#Client) redirect behavior does not follow the WHATWG Fetch standard. This is because it was written before established standards existed. As such, by modern standards, [Client](#Client) has a rather permissive behavior. For example, sensitive headers are retained on redirect to a subdomain or to a different scheme on the same host.

Any returned error will be of type [\*url.Error](https://pkg.go.dev/net/url#Error). The url.Error value's Timeout method will report true if the request timed out.

#### func (\*Client) [Get](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=486)

```
func (c *Client) Get(url string) (resp *Response, err error)
```

Get issues a GET to the specified URL. If the response is one of the following redirect codes, Get follows the redirect after calling the [Client.CheckRedirect](#Client.CheckRedirect) function:

```
301 (Moved Permanently)
302 (Found)
303 (See Other)
307 (Temporary Redirect)
308 (Permanent Redirect)
```

An error is returned if the [Client.CheckRedirect](#Client.CheckRedirect) function fails or if there was an HTTP protocol error. A non-2xx response doesn't cause an error. Any returned error will be of type [\*url.Error](https://pkg.go.dev/net/url#Error). The url.Error value's Timeout method will report true if the request timed out.

When err is nil, resp always contains a non-nil resp.Body. Caller should close resp.Body when done reading from it.

To make a request with custom headers, use [NewRequest](#NewRequest) and [Client.Do](#Client.Do).

To make a request with a specified context.Context, use [NewRequestWithContext](#NewRequestWithContext) and Client.Do.

#### func (\*Client) [Head](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=963)

```
func (c *Client) Head(url string) (resp *Response, err error)
```

Head issues a HEAD to the specified URL. If the response is one of the following redirect codes, Head follows the redirect after calling the [Client.CheckRedirect](#Client.CheckRedirect) function:

```
301 (Moved Permanently)
302 (Found)
303 (See Other)
307 (Temporary Redirect)
308 (Permanent Redirect)
```

To make a request with a specified [context.Context](https://pkg.go.dev/context#Context), use [NewRequestWithContext](#NewRequestWithContext) and [Client.Do](#Client.Do).

#### func (\*Client) [Post](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=886)

```
func (c *Client) Post(url, contentType string, body io.Reader) (resp *Response, err error)
```

Post issues a POST to the specified URL.

Caller should close resp.Body when done reading from it.

If the provided body is an [io.Closer](https://pkg.go.dev/io#Closer), it is closed after the request.

To set custom headers, use [NewRequest](#NewRequest) and [Client.Do](#Client.Do).

To make a request with a specified context.Context, use [NewRequestWithContext](#NewRequestWithContext) and [Client.Do](#Client.Do).

See the [Client.Do](#Client.Do) method documentation for details on how redirects are handled.

#### func (\*Client) [PostForm](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=929)

```
func (c *Client) PostForm(url string, data url.Values) (resp *Response, err error)
```

PostForm issues a POST to the specified URL, with data's keys and values URL-encoded as the request body.

The Content-Type header is set to application/x-www-form-urlencoded. To set other headers, use [NewRequest](#NewRequest) and [Client.Do](#Client.Do).

When err is nil, resp always contains a non-nil resp.Body. Caller should close resp.Body when done reading from it.

See the [Client.Do](#Client.Do) method documentation for details on how redirects are handled.

To make a request with a specified context.Context, use [NewRequestWithContext](#NewRequestWithContext) and Client.Do.


#### type [ClientConn](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=21) added in go1.26.0

```
type ClientConn struct {
	// contains filtered or unexported fields
}
```

A ClientConn is a client connection to an HTTP server.

Unlike a [Transport](#Transport), a ClientConn represents a single connection. Most users should use a Transport rather than creating client connections directly.

#### func (\*ClientConn) [Available](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=277) added in go1.26.0

```
func (cc *ClientConn) Available() int
```

Available reports the number of requests that may be sent to the connection without blocking. It returns 0 if the connection is closed.

#### func (\*ClientConn) [Close](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=210) added in go1.26.0

```
func (cc *ClientConn) Close() error
```

Close closes the connection. Outstanding RoundTrip calls are interrupted.

#### func (\*ClientConn) [Err](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=218) added in go1.26.0

```
func (cc *ClientConn) Err() error
```

Err reports any fatal connection errors. It returns nil if the connection is usable. If it returns non-nil, the connection can no longer be used.

#### func (\*ClientConn) [InFlight](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=284) added in go1.26.0

```
func (cc *ClientConn) InFlight() int
```

InFlight reports the number of requests in flight, including reserved requests. It returns 0 if the connection is closed.

#### func (\*ClientConn) [Release](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=308) added in go1.26.0

```
func (cc *ClientConn) Release()
```

Release releases an unused concurrency slot reserved by Reserve. If there are no reserved concurrency slots, it has no effect.

#### func (\*ClientConn) [Reserve](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=301) added in go1.26.0

```
func (cc *ClientConn) Reserve() error
```

Reserve reserves a concurrency slot on the connection. If Reserve returns nil, one additional RoundTrip call may be made without waiting for an existing request to complete.

The reserved concurrency slot is accounted as an in-flight request. A successful call to RoundTrip will decrement the Available count and increment the InFlight count.

Each successful call to Reserve should be followed by exactly one call to RoundTrip or Release, which will consume or release the reservation.

If the connection is closed or at its concurrency limit, Reserve returns an error.

#### func (\*ClientConn) [RoundTrip](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=254) added in go1.26.0

```
func (cc *ClientConn) RoundTrip(req *Request) (*Response, error)
```

RoundTrip implements the [RoundTripper](#RoundTripper) interface.

The request is sent on the client connection, regardless of the URL being requested or any proxy settings.

If the connection is at its concurrency limit, RoundTrip waits for the connection to become available before sending the request.

#### func (\*ClientConn) [SetStateHook](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=385) added in go1.26.0

```
func (cc *ClientConn) SetStateHook(f func(*ClientConn))
```

SetStateHook arranges for f to be called when the state of the connection changes. At most one call to f is made at a time. If the connection's state has changed since it was created, f is called immediately in a separate goroutine. f may be called synchronously from RoundTrip or Response.Body.Close.

If SetStateHook is called multiple times, the new hook replaces the old one. If f is nil, no further calls will be made to f after SetStateHook returns.

f is called when Available increases (more requests may be sent on the connection), InFlight decreases (existing requests complete), or Err begins returning non-nil (the connection is no longer usable).


####  type [CloseNotifier](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=218) deprecated added in go1.1

```
type CloseNotifier interface {
	// CloseNotify returns a channel that receives at most a
	// single value (true) when the client connection has gone
	// away.
	//
	// CloseNotify may wait to notify until Request.Body has been
	// fully read.
	//
	// After the Handler has returned, there is no guarantee
	// that the channel receives a value.
	//
	// If the protocol is HTTP/1.1 and CloseNotify is called while
	// processing an idempotent request (such as GET) while
	// HTTP/1.1 pipelining is in use, the arrival of a subsequent
	// pipelined request may cause a value to be sent on the
	// returned channel. In practice HTTP/1.1 pipelining is not
	// enabled in browsers and not seen often in the wild. If this
	// is a problem, use HTTP/2 or only use CloseNotify on methods
	// such as POST.
	CloseNotify() <-chan bool
}
```

The CloseNotifier interface is implemented by ResponseWriters which allow detecting when the underlying connection has gone away.

This mechanism can be used to cancel long operations on the server if the client has disconnected before the response is ready.

Deprecated: the CloseNotifier interface predates Go's context package. New code should use [Request.Context](#Request.Context) instead.


#### type [ConnState](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3339) added in go1.3

```
type ConnState int
```

A ConnState represents the state of a client connection to a server. It's used by the optional [Server.ConnState](#Server.ConnState) hook.

```
const (
	// StateNew represents a new connection that is expected to
	// send a request immediately. Connections begin at this
	// state and then transition to either StateActive or
	// StateClosed.
	StateNew ConnState = iota

	// StateActive represents a connection that has read 1 or more
	// bytes of a request. The Server.ConnState hook for
	// StateActive fires before the request has entered a handler
	// and doesn't fire again until the request has been
	// handled. After the request is handled, the state
	// transitions to StateClosed, StateHijacked, or StateIdle.
	// For HTTP/2, StateActive fires on the transition from zero
	// to one active request, and only transitions away once all
	// active requests are complete. That means that ConnState
	// cannot be used to do per-request work; ConnState only notes
	// the overall state of the connection.
	StateActive

	// StateIdle represents a connection that has finished
	// handling a request and is in the keep-alive state, waiting
	// for a new request. Connections transition from StateIdle
	// to either StateActive or StateClosed.
	StateIdle

	// StateHijacked represents a hijacked connection.
	// This is a terminal state. It does not transition to StateClosed.
	StateHijacked

	// StateClosed represents a closed connection.
	// This is a terminal state. Hijacked connections do not
	// transition to StateClosed.
	StateClosed
)
```

#### func (ConnState) [String](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3385) added in go1.3

```
func (c ConnState) String() string
```


#### type [Cookie](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/cookie.go;l=26)

```
type Cookie struct {
	Name   string
	Value  string
	Quoted bool // indicates whether the Value was originally quoted

	Path       string    // optional
	Domain     string    // optional
	Expires    time.Time // optional
	RawExpires string    // for reading cookies only

	// MaxAge=0 means no 'Max-Age' attribute specified.
	// MaxAge<0 means delete cookie now, equivalently 'Max-Age: 0'
	// MaxAge>0 means Max-Age attribute present and given in seconds
	MaxAge      int
	Secure      bool
	HttpOnly    bool
	SameSite    SameSite
	Partitioned bool
	Raw         string
	Unparsed    []string // Raw text of unparsed attribute-value pairs
}
```

A Cookie represents an HTTP cookie as sent in the Set-Cookie header of an HTTP response or the Cookie header of an HTTP request.

See [https://tools.ietf.org/html/rfc6265](https://tools.ietf.org/html/rfc6265) for details.

#### func [ParseCookie](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/cookie.go;l=91) added in go1.23.0

```
func ParseCookie(line string) ([]*Cookie, error)
```

ParseCookie parses a Cookie header value and returns all the cookies which were set in it. Since the same cookie name can appear multiple times the returned Values can contain more than one value for a given key.

#### func [ParseSetCookie](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/cookie.go;l=119) added in go1.23.0

```
func ParseSetCookie(line string) (*Cookie, error)
```

ParseSetCookie parses a Set-Cookie header value and returns a cookie. It returns an error on syntax error.

#### func (\*Cookie) [String](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/cookie.go;l=260)

```
func (c *Cookie) String() string
```

String returns the serialization of the cookie for use in a [Cookie](#Cookie) header (if only Name and Value are set) or a Set-Cookie response header (if other fields are set). If c is nil or c.Name is invalid, the empty string is returned.

#### func (\*Cookie) [Valid](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/cookie.go;l=327) added in go1.18

```
func (c *Cookie) Valid() error
```

Valid reports whether the cookie is valid.


#### type [CookieJar](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/jar.go;l=17)

```
type CookieJar interface {
	// SetCookies handles the receipt of the cookies in a reply for the
	// given URL.  It may or may not choose to save the cookies, depending
	// on the jar's policy and implementation.
	SetCookies(u *url.URL, cookies []*Cookie)

	// Cookies returns the cookies to send in a request for the given URL.
	// It is up to the implementation to honor the standard cookie use
	// restrictions such as in RFC 6265.
	Cookies(u *url.URL) []*Cookie
}
```

A CookieJar manages storage and use of cookies in HTTP requests.

Implementations of CookieJar must be safe for concurrent use by multiple goroutines.

The net/http/cookiejar package provides a CookieJar implementation.

#### type [CrossOriginProtection](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/csrf.go;l=36) added in go1.25.0

```
type CrossOriginProtection struct {
	// contains filtered or unexported fields
}
```

CrossOriginProtection implements protections against [Cross-Site Request Forgery (CSRF)](https://developer.mozilla.org/en-US/docs/Web/Security/Attacks/CSRF) by rejecting non-safe cross-origin browser requests.

Cross-origin requests are currently detected with the [Sec-Fetch-Site](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Sec-Fetch-Site) header, available in all browsers since 2023, or by comparing the hostname of the [Origin](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Origin) header with the Host header.

The GET, HEAD, and OPTIONS methods are [safe methods](https://developer.mozilla.org/en-US/docs/Glossary/Safe/HTTP) and are always allowed. It's important that applications do not perform any state changing actions due to requests with safe methods.

Requests without Sec-Fetch-Site or Origin headers are currently assumed to be either same-origin or non-browser requests, and are allowed.

The zero value of CrossOriginProtection is valid and has no trusted origins or bypass patterns.

Example [¶](#example-CrossOriginProtection "Go to Example")

```
package main

import (
	"io"
	"log"
	"net/http"
	"time"
)

func main() {
	mux := http.NewServeMux()

	mux.HandleFunc("/hello", func(w http.ResponseWriter, req *http.Request) {
		io.WriteString(w, "request allowed\n")
	})

	srv := http.Server{
		Addr:         ":8080",
		ReadTimeout:  15 * time.Second,
		WriteTimeout: 15 * time.Second,
		// Use CrossOriginProtection.Handler to block all non-safe cross-origin
		// browser requests to mux.
		Handler: http.NewCrossOriginProtection().Handler(mux),
	}

	log.Fatal(srv.ListenAndServe())
}
```


#### func [NewCrossOriginProtection](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/csrf.go;l=44) added in go1.25.0

```
func NewCrossOriginProtection() *CrossOriginProtection
```

NewCrossOriginProtection returns a new [CrossOriginProtection](#CrossOriginProtection) value.

#### func (\*CrossOriginProtection) [AddInsecureBypassPattern](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/csrf.go;l=99) added in go1.25.0

```
func (c *CrossOriginProtection) AddInsecureBypassPattern(pattern string)
```

AddInsecureBypassPattern permits all requests that match the given pattern.

The pattern syntax and precedence rules are the same as [ServeMux](#ServeMux). Only requests that match the pattern directly are permitted. Those that ServeMux would redirect to a pattern (e.g. after cleaning the path or adding a trailing slash) are not.

AddInsecureBypassPattern panics if the pattern conflicts with one already registered, or if the pattern is syntactically invalid (for example, an improperly formed wildcard).

AddInsecureBypassPattern can be called concurrently with other methods or request handling, and applies to future requests.

#### func (\*CrossOriginProtection) [AddTrustedOrigin](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/csrf.go;l=57) added in go1.25.0

```
func (c *CrossOriginProtection) AddTrustedOrigin(origin string) error
```

AddTrustedOrigin allows all requests with an [Origin](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Origin) header which exactly matches the given value.

Origin header values are of the form "scheme://host\[:port\]".

AddTrustedOrigin can be called concurrently with other methods or request handling, and applies to future requests.

#### func (\*CrossOriginProtection) [Check](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/csrf.go;l=134) added in go1.25.0

```
func (c *CrossOriginProtection) Check(req *Request) error
```

Check applies cross-origin checks to a request. It returns an error if the request should be rejected.

#### func (\*CrossOriginProtection) [Handler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/csrf.go;l=206) added in go1.25.0

```
func (c *CrossOriginProtection) Handler(h Handler) Handler
```

Handler returns a handler that applies cross-origin checks before invoking the handler h.

If a request fails cross-origin checks, the request is rejected with a 403 Forbidden status or handled with the handler passed to [CrossOriginProtection.SetDenyHandler](#CrossOriginProtection.SetDenyHandler).

#### func (\*CrossOriginProtection) [SetDenyHandler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/csrf.go;l=124) added in go1.25.0

```
func (c *CrossOriginProtection) SetDenyHandler(h Handler)
```

SetDenyHandler sets a handler to invoke when a request is rejected. The default error handler responds with a 403 Forbidden status.

SetDenyHandler can be called concurrently with other methods or request handling, and applies to future requests.

Check does not call the error handler.


#### type [Dir](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=45)

```
type Dir string
```

A Dir implements [FileSystem](#FileSystem) using the native file system restricted to a specific directory tree.

While the [FileSystem.Open](#FileSystem.Open) method takes '/'-separated paths, a Dir's string value is a directory path on the native file system, not a URL, so it is separated by [filepath.Separator](https://pkg.go.dev/path/filepath#Separator), which isn't necessarily '/'.

Note that Dir could expose sensitive files and directories. Dir will follow symlinks pointing out of the directory tree, which can be especially dangerous if serving from a directory in which users are able to create arbitrary symlinks. Dir will also allow access to files and directories starting with a period, which could expose sensitive directories like .git or sensitive files like .htpasswd. To exclude files with a leading period, remove the files/directories from the server or create a custom FileSystem implementation.

An empty Dir is treated as ".".

#### func (Dir) [Open](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=78)

```
func (d Dir) Open(name string) (File, error)
```

Open implements [FileSystem](#FileSystem) using [os.Open](https://pkg.go.dev/os#Open), opening files for reading rooted and relative to the directory d.


#### type [File](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=114)

```
type File interface {
	io.Closer
	io.Reader
	io.Seeker
	Readdir(count int) ([]fs.FileInfo, error)
	Stat() (fs.FileInfo, error)
}
```

A File is returned by a [FileSystem](#FileSystem)'s Open method and can be served by the [FileServer](#FileServer) implementation.

The methods should behave the same as those on an [\*os.File](https://pkg.go.dev/os#File).

#### type [FileSystem](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=106)

```
type FileSystem interface {
	Open(name string) (File, error)
}
```

A FileSystem implements access to a collection of named files. The elements in a file path are separated by slash ('/', U+002F) characters, regardless of host operating system convention. See the [FileServer](#FileServer) function to convert a FileSystem to a [Handler](#Handler).

This interface predates the [fs.FS](https://pkg.go.dev/io/fs#FS) interface, which can be used instead: the [FS](#FS) adapter function converts an fs.FS to a FileSystem.

#### func [FS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=961) added in go1.16

```
func FS(fsys fs.FS) FileSystem
```

FS converts fsys to a [FileSystem](#FileSystem) implementation, for use with [FileServer](#FileServer) and [NewFileTransport](#NewFileTransport). The files provided by fsys must implement [io.Seeker](https://pkg.go.dev/io#Seeker).


#### type [Flusher](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=175)

```
type Flusher interface {
	// Flush sends any buffered data to the client.
	Flush()
}
```

The Flusher interface is implemented by ResponseWriters that allow an HTTP handler to flush buffered data to the client.

The default HTTP/1.x and HTTP/2 [ResponseWriter](#ResponseWriter) implementations support [Flusher](#Flusher), but ResponseWriter wrappers may not. Handlers should always test for this ability at runtime.

Note that even for ResponseWriters that support Flush, if the client is connected through an HTTP proxy, the buffered data may not reach the client until the response completes.

#### type [HTTP2Config](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=249) added in go1.24.0

```
type HTTP2Config struct {
	// MaxConcurrentStreams optionally specifies the number of
	// concurrent streams that a client may have open at a time.
	// If zero, MaxConcurrentStreams defaults to at least 100.
	//
	// This parameter only applies to Servers.
	MaxConcurrentStreams int

	// StrictMaxConcurrentRequests controls whether an HTTP/2 server's
	// concurrency limit should be respected across all connections
	// to that server.
	// If true, new requests sent when a connection's concurrency limit
	// has been exceeded will block until an existing request completes.
	// If false, an additional connection will be opened if all
	// existing connections are at their limit.
	//
	// This parameter only applies to Transports.
	StrictMaxConcurrentRequests bool

	// MaxDecoderHeaderTableSize optionally specifies an upper limit for the
	// size of the header compression table used for decoding headers sent
	// by the peer.
	// A valid value is less than 4MiB.
	// If zero or invalid, a default value is used.
	MaxDecoderHeaderTableSize int

	// MaxEncoderHeaderTableSize optionally specifies an upper limit for the
	// header compression table used for sending headers to the peer.
	// A valid value is less than 4MiB.
	// If zero or invalid, a default value is used.
	MaxEncoderHeaderTableSize int

	// MaxReadFrameSize optionally specifies the largest frame
	// this endpoint is willing to read.
	// A valid value is between 16KiB and 16MiB, inclusive.
	// If zero or invalid, a default value is used.
	MaxReadFrameSize int

	// MaxReceiveBufferPerConnection is the maximum size of the
	// flow control window for data received on a connection.
	// A valid value is at least 64KiB and less than 4MiB.
	// If invalid, a default value is used.
	MaxReceiveBufferPerConnection int

	// MaxReceiveBufferPerStream is the maximum size of
	// the flow control window for data received on a stream (request).
	// A valid value is less than 4MiB.
	// If zero or invalid, a default value is used.
	MaxReceiveBufferPerStream int

	// SendPingTimeout is the timeout after which a health check using a ping
	// frame will be carried out if no frame is received on a connection.
	// If zero, no health check is performed.
	SendPingTimeout time.Duration

	// PingTimeout is the timeout after which a connection will be closed
	// if a response to a ping is not received.
	// If zero, a default of 15 seconds is used.
	PingTimeout time.Duration

	// WriteByteTimeout is the timeout after which a connection will be
	// closed if no data can be written to it. The timeout begins when data is
	// available to write, and is extended whenever any bytes are written.
	WriteByteTimeout time.Duration

	// PermitProhibitedCipherSuites, if true, permits the use of
	// cipher suites prohibited by the HTTP/2 spec.
	PermitProhibitedCipherSuites bool

	// CountError, if non-nil, is called on HTTP/2 errors.
	// It is intended to increment a metric for monitoring.
	// The errType contains only lowercase letters, digits, and underscores
	// (a-z, 0-9, _).
	CountError func(errType string)
}
```

HTTP2Config defines HTTP/2 configuration parameters common to both [Transport](#Transport) and [Server](#Server).

#### type [Handler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=89)

```
type Handler interface {
	ServeHTTP(ResponseWriter, *Request)
}
```

A Handler responds to an HTTP request.

[Handler.ServeHTTP](#Handler.ServeHTTP) should write reply headers and data to the [ResponseWriter](#ResponseWriter) and then return. Returning signals that the request is finished; it is not valid to use the [ResponseWriter](#ResponseWriter) or read from the [Request.Body](#Request.Body) after or concurrently with the completion of the ServeHTTP call.

Depending on the HTTP client software, HTTP protocol version, and any intermediaries between the client and the Go server, it may not be possible to read from the [Request.Body](#Request.Body) after writing to the [ResponseWriter](#ResponseWriter). Cautious handlers should read the [Request.Body](#Request.Body) first, and then reply.

Except for reading the body, handlers should not modify the provided Request.

If ServeHTTP panics, the server (the caller of ServeHTTP) assumes that the effect of the panic was isolated to the active request. It recovers the panic, logs a stack trace to the server error log, and either closes the network connection or sends an HTTP/2 RST\_STREAM, depending on the HTTP protocol. To abort a handler so the client sees an interrupted response but the server doesn't log an error, panic with the value [ErrAbortHandler](#ErrAbortHandler).

#### func [AllowQuerySemicolons](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3427) added in go1.17

```
func AllowQuerySemicolons(h Handler) Handler
```

AllowQuerySemicolons returns a handler that serves requests by converting any unescaped semicolons in the URL query to ampersands, and invoking the handler h.

This restores the pre-Go 1.17 behavior of splitting query parameters on both semicolons and ampersands. (See golang.org/issue/25192). Note that this behavior doesn't match that of many proxies, and the mismatch can lead to security issues.

AllowQuerySemicolons should be invoked before [Request.ParseForm](#Request.ParseForm) is called.

#### func [FileServer](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=978)

```
func FileServer(root FileSystem) Handler
```

FileServer returns a handler that serves HTTP requests with the contents of the file system rooted at root.

As a special case, the returned file server redirects any request ending in "/index.html" to the same path, without the final "index.html".

To use the operating system's file system implementation, use [http.Dir](#Dir):

```
http.Handle("/", http.FileServer(http.Dir("/tmp")))
```

To use an [fs.FS](https://pkg.go.dev/io/fs#FS) implementation, use [http.FileServerFS](#FileServerFS) instead.

Example [¶](#example-FileServer "Go to Example")

```
package main

import (
	"log"
	"net/http"
)

func main() {
	// Simple static webserver:
	log.Fatal(http.ListenAndServe(":8080", http.FileServer(http.Dir("/usr/share/doc"))))
}
```


Example (DotFileHiding) [¶](#example-FileServer-DotFileHiding "Go to Example (DotFileHiding)")

```
package main

import (
	"io"
	"io/fs"
	"log"
	"net/http"
	"strings"
)

// containsDotFile reports whether name contains a path element starting with a period.
// The name is assumed to be a delimited by forward slashes, as guaranteed
// by the http.FileSystem interface.
func containsDotFile(name string) bool {
	parts := strings.Split(name, "/")
	for _, part := range parts {
		if strings.HasPrefix(part, ".") {
			return true
		}
	}
	return false
}

// dotFileHidingFile is the http.File use in dotFileHidingFileSystem.
// It is used to wrap the Readdir method of http.File so that we can
// remove files and directories that start with a period from its output.
type dotFileHidingFile struct {
	http.File
}

// Readdir is a wrapper around the Readdir method of the embedded File
// that filters out all files that start with a period in their name.
func (f dotFileHidingFile) Readdir(n int) (fis []fs.FileInfo, err error) {
	files, err := f.File.Readdir(n)
	for _, file := range files { // Filters out the dot files
		if !strings.HasPrefix(file.Name(), ".") {
			fis = append(fis, file)
		}
	}
	if err == nil && n > 0 && len(fis) == 0 {
		err = io.EOF
	}
	return
}

// dotFileHidingFileSystem is an http.FileSystem that hides
// hidden "dot files" from being served.
type dotFileHidingFileSystem struct {
	http.FileSystem
}

// Open is a wrapper around the Open method of the embedded FileSystem
// that serves a 403 permission error when name has a file or directory
// with whose name starts with a period in its path.
func (fsys dotFileHidingFileSystem) Open(name string) (http.File, error) {
	if containsDotFile(name) { // If dot file, return 403 response
		return nil, fs.ErrPermission
	}

	file, err := fsys.FileSystem.Open(name)
	if err != nil {
		return nil, err
	}
	return dotFileHidingFile{file}, nil
}

func main() {
	fsys := dotFileHidingFileSystem{http.Dir(".")}
	http.Handle("/", http.FileServer(fsys))
	log.Fatal(http.ListenAndServe(":8080", nil))
}
```


Example (StripPrefix) [¶](#example-FileServer-StripPrefix "Go to Example (StripPrefix)")

```
package main

import (
	"net/http"
)

func main() {
	// To serve a directory on disk (/tmp) under an alternate URL
	// path (/tmpfiles/), use StripPrefix to modify the request
	// URL's path before the FileServer sees it:
	http.Handle("/tmpfiles/", http.StripPrefix("/tmpfiles/", http.FileServer(http.Dir("/tmp"))))
}
```



#### func [FileServerFS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/fs.go;l=991) added in go1.22.0

```
func FileServerFS(root fs.FS) Handler
```

FileServerFS returns a handler that serves HTTP requests with the contents of the file system fsys. The files provided by fsys must implement [io.Seeker](https://pkg.go.dev/io#Seeker).

As a special case, the returned file server redirects any request ending in "/index.html" to the same path, without the final "index.html".

```
http.Handle("/", http.FileServerFS(fsys))
```

#### func [MaxBytesHandler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=4286) added in go1.18

```
func MaxBytesHandler(h Handler, n int64) Handler
```

MaxBytesHandler returns a [Handler](#Handler) that runs h with its [ResponseWriter](#ResponseWriter) and [Request.Body](#Request.Body) wrapped by a MaxBytesReader.

#### func [NotFoundHandler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2378)

```
func NotFoundHandler() Handler
```

NotFoundHandler returns a simple request handler that replies to each request with a “404 page not found” reply.

Example [¶](#example-NotFoundHandler "Go to Example")

```
package main

import (
	"fmt"
	"log"
	"net/http"
)

func newPeopleHandler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintln(w, "This is the people handler.")
	})
}

func main() {
	mux := http.NewServeMux()

	// Create sample handler to returns 404
	mux.Handle("/resources", http.NotFoundHandler())

	// Create sample handler that returns 200
	mux.Handle("/resources/people/", newPeopleHandler())

	log.Fatal(http.ListenAndServe(":8080", mux))
}
```



#### func [RedirectHandler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2504)

```
func RedirectHandler(url string, code int) Handler
```

RedirectHandler returns a request handler that redirects each request it receives to the given url using the given status code.

The provided code should be in the 3xx range and is usually [StatusMovedPermanently](#StatusMovedPermanently), [StatusFound](#StatusFound) or [StatusSeeOther](#StatusSeeOther).

#### func [StripPrefix](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2386)

```
func StripPrefix(prefix string, h Handler) Handler
```

StripPrefix returns a handler that serves HTTP requests by removing the given prefix from the request URL's Path (and RawPath if set) and invoking the handler h. StripPrefix handles a request for a path that doesn't begin with prefix by replying with an HTTP 404 not found error. The prefix must match exactly: if the prefix in the request contains escaped characters the reply is also an HTTP 404 not found error.

Example [¶](#example-StripPrefix "Go to Example")

```
package main

import (
	"net/http"
)

func main() {
	// To serve a directory on disk (/tmp) under an alternate URL
	// path (/tmpfiles/), use StripPrefix to modify the request
	// URL's path before the FileServer sees it:
	http.Handle("/tmpfiles/", http.StripPrefix("/tmpfiles/", http.FileServer(http.Dir("/tmp"))))
}
```



#### func [TimeoutHandler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=4006)

```
func TimeoutHandler(h Handler, dt time.Duration, msg string) Handler
```

TimeoutHandler returns a [Handler](#Handler) that runs h with the given time limit.

The new Handler calls h.ServeHTTP to handle each request, but if a call runs for longer than its time limit, the handler responds with a 503 Service Unavailable error and the given message in its body. (If msg is empty, a suitable default message will be sent.) After such a timeout, writes by h to its [ResponseWriter](#ResponseWriter) will return [ErrHandlerTimeout](#ErrHandlerTimeout).

TimeoutHandler supports the [Pusher](#Pusher) interface but does not support the [Hijacker](#Hijacker) or [Flusher](#Flusher) interfaces.


#### type [HandlerFunc](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2334)

```
type HandlerFunc func(ResponseWriter, *Request)
```

The HandlerFunc type is an adapter to allow the use of ordinary functions as HTTP handlers. If f is a function with the appropriate signature, HandlerFunc(f) is a [Handler](#Handler) that calls f.

#### func (HandlerFunc) [ServeHTTP](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2337)

```
func (f HandlerFunc) ServeHTTP(w ResponseWriter, r *Request)
```

ServeHTTP calls f(w, r).


#### type [Header](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=24)

```
type Header map[string][]string
```

A Header represents the key-value pairs in an HTTP header.

The keys should be in canonical form, as returned by [CanonicalHeaderKey](#CanonicalHeaderKey).

#### func (Header) [Add](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=30)

```
func (h Header) Add(key, value string)
```

Add adds the key, value pair to the header. It appends to any existing values associated with key. The key is case insensitive; it is canonicalized by [CanonicalHeaderKey](#CanonicalHeaderKey).

#### func (Header) [Clone](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=94) added in go1.13

```
func (h Header) Clone() Header
```

Clone returns a copy of h or nil if h is nil.

#### func (Header) [Del](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=80)

```
func (h Header) Del(key string)
```

Del deletes the values associated with key. The key is case insensitive; it is canonicalized by [CanonicalHeaderKey](#CanonicalHeaderKey).

#### func (Header) [Get](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=49)

```
func (h Header) Get(key string) string
```

Get gets the first value associated with the given key. If there are no values associated with the key, Get returns "". It is case insensitive; [textproto.CanonicalMIMEHeaderKey](https://pkg.go.dev/net/textproto#CanonicalMIMEHeaderKey) is used to canonicalize the provided key. Get assumes that all keys are stored in canonical form. To use non-canonical keys, access the map directly.

#### func (Header) [Set](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=39)

```
func (h Header) Set(key, value string)
```

Set sets the header entries associated with key to the single element value. It replaces any existing values associated with key. The key is case insensitive; it is canonicalized by [textproto.CanonicalMIMEHeaderKey](https://pkg.go.dev/net/textproto#CanonicalMIMEHeaderKey). To use non-canonical keys, assign to the map directly.

#### func (Header) [Values](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=58) added in go1.14

```
func (h Header) Values(key string) []string
```

Values returns all values associated with the given key. It is case insensitive; [textproto.CanonicalMIMEHeaderKey](https://pkg.go.dev/net/textproto#CanonicalMIMEHeaderKey) is used to canonicalize the provided key. To use non-canonical keys, access the map directly. The returned slice is not a copy.

#### func (Header) [Write](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=85)

```
func (h Header) Write(w io.Writer) error
```

Write writes a header in wire format.

#### func (Header) [WriteSubset](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/header.go;l=186)

```
func (h Header) WriteSubset(w io.Writer, exclude map[string]bool) error
```

WriteSubset writes a header in wire format. If exclude is not nil, keys where exclude\[key\] == true are not written. Keys are not canonicalized before checking the exclude map.


#### type [Hijacker](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=187)

```
type Hijacker interface {
	// Hijack lets the caller take over the connection.
	// After a call to Hijack the HTTP server library
	// will not do anything else with the connection.
	//
	// It becomes the caller's responsibility to manage
	// and close the connection.
	//
	// The returned net.Conn may have read or write deadlines
	// already set, depending on the configuration of the
	// Server. It is the caller's responsibility to set
	// or clear those deadlines as needed.
	//
	// The returned bufio.Reader may contain unprocessed buffered
	// data from the client.
	//
	// After a call to Hijack, the original Request.Body must not
	// be used. The original Request's Context remains valid and
	// is not canceled until the Request's ServeHTTP method
	// returns.
	Hijack() (net.Conn, *bufio.ReadWriter, error)
}
```

The Hijacker interface is implemented by ResponseWriters that allow an HTTP handler to take over the connection.

The default [ResponseWriter](#ResponseWriter) for HTTP/1.x connections supports Hijacker, but HTTP/2 connections intentionally do not. ResponseWriter wrappers may also not support Hijacker. Handlers should always test for this ability at runtime.

Example [¶](#example-Hijacker "Go to Example")

```
package main

import (
	"fmt"
	"log"
	"net/http"
)

func main() {
	http.HandleFunc("/hijack", func(w http.ResponseWriter, r *http.Request) {
		hj, ok := w.(http.Hijacker)
		if !ok {
			http.Error(w, "webserver doesn't support hijacking", http.StatusInternalServerError)
			return
		}
		conn, bufrw, err := hj.Hijack()
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		// Don't forget to close the connection:
		defer conn.Close()
		bufrw.WriteString("Now we're speaking raw TCP. Say hi: ")
		bufrw.Flush()
		s, err := bufrw.ReadString('\n')
		if err != nil {
			log.Printf("error reading string: %v", err)
			return
		}
		fmt.Fprintf(bufrw, "You said: %q\nBye.\n", s)
		bufrw.Flush()
	})
}
```



#### type [MaxBytesError](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1217) added in go1.19

```
type MaxBytesError struct {
	Limit int64
}
```

MaxBytesError is returned by [MaxBytesReader](#MaxBytesReader) when its read limit is exceeded.

#### func (\*MaxBytesError) [Error](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1221) added in go1.19

```
func (e *MaxBytesError) Error() string
```


####  type [ProtocolError](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=48) deprecated

```
type ProtocolError struct {
	ErrorString string
}
```

ProtocolError represents an HTTP protocol error.

Deprecated: Not all errors in the http package related to protocol errors are of type ProtocolError.

#### func (\*ProtocolError) [Error](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=52)

```
func (pe *ProtocolError) Error() string
```

#### func (\*ProtocolError) [Is](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=55) added in go1.21.0

```
func (pe *ProtocolError) Is(err error) bool
```

Is lets http.ErrNotSupported match errors.ErrUnsupported.



#### type [Protocols](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=29) added in go1.24.0

```
type Protocols struct {
	// contains filtered or unexported fields
}
```

Protocols is a set of HTTP protocols. The zero value is an empty set of protocols.

The supported protocols are:

- HTTP1 is the HTTP/1.0 and HTTP/1.1 protocols. HTTP1 is supported on both unsecured TCP and secured TLS connections.
- HTTP2 is the HTTP/2 protocol over a TLS connection.
- UnencryptedHTTP2 is the HTTP/2 protocol over an unsecured TCP connection.

Example (Http1) [¶](#example-Protocols-Http1 "Go to Example (Http1)")

```
package main

import (
	"log"
	"net/http"
)

func main() {
	srv := http.Server{
		Addr: ":8443",
	}

	// Serve only HTTP/1.
	srv.Protocols = new(http.Protocols)
	srv.Protocols.SetHTTP1(true)

	log.Fatal(srv.ListenAndServeTLS("cert.pem", "key.pem"))
}
```


Example (Http1or2) [¶](#example-Protocols-Http1or2 "Go to Example (Http1or2)")

```
package main

import (
	"log"
	"net/http"
)

func main() {
	t := http.DefaultTransport.(*http.Transport).Clone()

	// Use either HTTP/1 and HTTP/2.
	t.Protocols = new(http.Protocols)
	t.Protocols.SetHTTP1(true)
	t.Protocols.SetHTTP2(true)

	cli := &http.Client{Transport: t}
	res, err := cli.Get("http://www.google.com/robots.txt")
	if err != nil {
		log.Fatal(err)
	}
	res.Body.Close()
}
```


#### func (Protocols) [HTTP1](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=41) added in go1.24.0

```
func (p Protocols) HTTP1() bool
```

HTTP1 reports whether p includes HTTP/1.

#### func (Protocols) [HTTP2](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=47) added in go1.24.0

```
func (p Protocols) HTTP2() bool
```

HTTP2 reports whether p includes HTTP/2.

#### func (\*Protocols) [SetHTTP1](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=44) added in go1.24.0

```
func (p *Protocols) SetHTTP1(ok bool)
```

SetHTTP1 adds or removes HTTP/1 from p.

#### func (\*Protocols) [SetHTTP2](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=50) added in go1.24.0

```
func (p *Protocols) SetHTTP2(ok bool)
```

SetHTTP2 adds or removes HTTP/2 from p.

#### func (\*Protocols) [SetUnencryptedHTTP2](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=56) added in go1.24.0

```
func (p *Protocols) SetUnencryptedHTTP2(ok bool)
```

SetUnencryptedHTTP2 adds or removes unencrypted HTTP/2 from p.

#### func (Protocols) [String](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=80) added in go1.24.0

```
func (p Protocols) String() string
```

#### func (Protocols) [UnencryptedHTTP2](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=53) added in go1.24.0

```
func (p Protocols) UnencryptedHTTP2() bool
```

UnencryptedHTTP2 reports whether p includes unencrypted HTTP/2.


#### type [PushOptions](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=205) added in go1.8

```
type PushOptions struct {
	// Method specifies the HTTP method for the promised request.
	// If set, it must be "GET" or "HEAD". Empty means "GET".
	Method string

	// Header specifies additional promised request headers. This cannot
	// include HTTP/2 pseudo header fields like ":path" and ":scheme",
	// which will be added automatically.
	Header Header
}
```

PushOptions describes options for [Pusher.Push](#Pusher.Push).

#### type [Pusher](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/http.go;l=219) added in go1.8

```
type Pusher interface {
	// Push initiates an HTTP/2 server push. This constructs a synthetic
	// request using the given target and options, serializes that request
	// into a PUSH_PROMISE frame, then dispatches that request using the
	// server's request handler. If opts is nil, default options are used.
	//
	// The target must either be an absolute path (like "/path") or an absolute
	// URL that contains a valid host and the same scheme as the parent request.
	// If the target is a path, it will inherit the scheme and host of the
	// parent request.
	//
	// The HTTP/2 spec disallows recursive pushes and cross-authority pushes.
	// Push may or may not detect these invalid pushes; however, invalid
	// pushes will be detected and canceled by conforming clients.
	//
	// Handlers that wish to push URL X should call Push before sending any
	// data that may trigger a request for URL X. This avoids a race where the
	// client issues requests for X before receiving the PUSH_PROMISE for X.
	//
	// Push will run in a separate goroutine making the order of arrival
	// non-deterministic. Any required synchronization needs to be implemented
	// by the caller.
	//
	// Push returns ErrNotSupported if the client has disabled push or if push
	// is not supported on the underlying connection.
	Push(target string, opts *PushOptions) error
}
```

Pusher is the interface implemented by ResponseWriters that support HTTP/2 server push. For more background, see [https://tools.ietf.org/html/rfc7540#section-8.2](https://tools.ietf.org/html/rfc7540#section-8.2).

#### type [Request](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=114)

```
type Request struct {
	// Method specifies the HTTP method (GET, POST, PUT, etc.).
	// For client requests, an empty string means GET.
	Method string

	// URL specifies either the URI being requested (for server
	// requests) or the URL to access (for client requests).
	//
	// For server requests, the URL is parsed from the URI
	// supplied on the Request-Line as stored in RequestURI.  For
	// most requests, fields other than Path and RawQuery will be
	// empty. (See RFC 7230, Section 5.3)
	//
	// For client requests, the URL's Host specifies the server to
	// connect to, while the Request's Host field optionally
	// specifies the Host header value to send in the HTTP
	// request.
	URL *url.URL

	// The protocol version for incoming server requests.
	//
	// For client requests, these fields are ignored. The HTTP
	// client code always uses either HTTP/1.1 or HTTP/2.
	// See the docs on Transport for details.
	Proto      string // "HTTP/1.0"
	ProtoMajor int    // 1
	ProtoMinor int    // 0

	// Header contains the request header fields either received
	// by the server or to be sent by the client.
	//
	// If a server received a request with header lines,
	//
	//	Host: example.com
	//	accept-encoding: gzip, deflate
	//	Accept-Language: en-us
	//	fOO: Bar
	//	foo: two
	//
	// then
	//
	//	Header = map[string][]string{
	//		"Accept-Encoding": {"gzip, deflate"},
	//		"Accept-Language": {"en-us"},
	//		"Foo": {"Bar", "two"},
	//	}
	//
	// For incoming requests, the Host header is promoted to the
	// Request.Host field and removed from the Header map.
	//
	// HTTP defines that header names are case-insensitive. The
	// request parser implements this by using CanonicalHeaderKey,
	// making the first character and any characters following a
	// hyphen uppercase and the rest lowercase.
	//
	// For client requests, certain headers such as Content-Length
	// and Connection are automatically written when needed and
	// values in Header may be ignored. See the documentation
	// for the Request.Write method.
	Header Header

	// Body is the request's body.
	//
	// For client requests, a nil body means the request has no
	// body, such as a GET request. The HTTP Client's Transport
	// is responsible for calling the Close method.
	//
	// For server requests, the Request Body is always non-nil
	// but will return EOF immediately when no body is present.
	// The Server will close the request body. The ServeHTTP
	// Handler does not need to.
	//
	// Body must allow Read to be called concurrently with Close.
	// In particular, calling Close should unblock a Read waiting
	// for input.
	Body io.ReadCloser

	// GetBody defines an optional func to return a new copy of
	// Body. It is used for client requests when a redirect requires
	// reading the body more than once. Use of GetBody still
	// requires setting Body.
	//
	// For server requests, it is unused.
	GetBody func() (io.ReadCloser, error)

	// ContentLength records the length of the associated content.
	// The value -1 indicates that the length is unknown.
	// Values >= 0 indicate that the given number of bytes may
	// be read from Body.
	//
	// For client requests, a value of 0 with a non-nil Body is
	// also treated as unknown.
	ContentLength int64

	// TransferEncoding lists the transfer encodings from outermost to
	// innermost. An empty list denotes the "identity" encoding.
	// TransferEncoding can usually be ignored; chunked encoding is
	// automatically added and removed as necessary when sending and
	// receiving requests.
	TransferEncoding []string

	// Close indicates whether to close the connection after
	// replying to this request (for servers) or after sending this
	// request and reading its response (for clients).
	//
	// For server requests, the HTTP server handles this automatically
	// and this field is not needed by Handlers.
	//
	// For client requests, setting this field prevents re-use of
	// TCP connections between requests to the same hosts, as if
	// Transport.DisableKeepAlives were set.
	Close bool

	// For server requests, Host specifies the host on which the
	// URL is sought. For HTTP/1 (per RFC 7230, section 5.4), this
	// is either the value of the "Host" header or the host name
	// given in the URL itself. For HTTP/2, it is the value of the
	// ":authority" pseudo-header field.
	// It may be of the form "host:port". For international domain
	// names, Host may be in Punycode or Unicode form. Use
	// golang.org/x/net/idna to convert it to either format if
	// needed.
	// To prevent DNS rebinding attacks, server Handlers should
	// validate that the Host header has a value for which the
	// Handler considers itself authoritative. The included
	// ServeMux supports patterns registered to particular host
	// names and thus protects its registered Handlers.
	//
	// For client requests, Host optionally overrides the Host
	// header to send. If empty, the Request.Write method uses
	// the value of URL.Host. Host may contain an international
	// domain name.
	Host string

	// Form contains the parsed form data, including both the URL
	// field's query parameters and the PATCH, POST, or PUT form data.
	// This field is only available after ParseForm is called.
	// The HTTP client ignores Form and uses Body instead.
	Form url.Values

	// PostForm contains the parsed form data from PATCH, POST
	// or PUT body parameters.
	//
	// This field is only available after ParseForm is called.
	// The HTTP client ignores PostForm and uses Body instead.
	PostForm url.Values

	// MultipartForm is the parsed multipart form, including file uploads.
	// This field is only available after ParseMultipartForm is called.
	// The HTTP client ignores MultipartForm and uses Body instead.
	MultipartForm *multipart.Form

	// Trailer specifies additional headers that are sent after the request
	// body.
	//
	// For server requests, the Trailer map initially contains only the
	// trailer keys, with nil values. (The client declares which trailers it
	// will later send.)  While the handler is reading from Body, it must
	// not reference Trailer. After reading from Body returns EOF, Trailer
	// can be read again and will contain non-nil values, if they were sent
	// by the client.
	//
	// For client requests, Trailer must be initialized to a map containing
	// the trailer keys to later send. The values may be nil or their final
	// values. The ContentLength must be 0 or -1, to send a chunked request.
	// After the HTTP request is sent the map values can be updated while
	// the request body is read. Once the body returns EOF, the caller must
	// not mutate Trailer.
	//
	// Writing a request whose Trailer contains a key with invalid bytes
	// (such as CR or LF), or such a value present when Write begins,
	// returns an error.
	//
	// Few HTTP clients, servers, or proxies support HTTP trailers.
	Trailer Header

	// RemoteAddr allows HTTP servers and other software to record
	// the network address that sent the request, usually for
	// logging. This field is not filled in by ReadRequest and
	// has no defined format. The HTTP server in this package
	// sets RemoteAddr to an "IP:port" address before invoking a
	// handler.
	// This field is ignored by the HTTP client.
	RemoteAddr string

	// RequestURI is the unmodified request-target of the
	// Request-Line (RFC 7230, Section 3.1.1) as sent by the client
	// to a server. Usually the URL field should be used instead.
	// It is an error to set this field in an HTTP client request.
	RequestURI string

	// TLS allows HTTP servers and other software to record
	// information about the TLS connection on which the request
	// was received. This field is not filled in by ReadRequest.
	// The HTTP server in this package sets the field for
	// TLS-enabled connections before invoking a handler;
	// otherwise it leaves the field nil.
	// This field is ignored by the HTTP client.
	TLS *tls.ConnectionState

	// Cancel is an optional channel whose closure indicates that the client
	// request should be regarded as canceled. Not all implementations of
	// RoundTripper may support Cancel.
	//
	// For server requests, this field is not applicable.
	//
	// Deprecated: Set the Request's context with NewRequestWithContext
	// instead. If a Request's Cancel field and context are both
	// set, it is undefined whether Cancel is respected.
	Cancel <-chan struct{}

	// Response is the redirect response which caused this request
	// to be created. This field is only populated during client
	// redirects.
	Response *Response

	// Pattern is the [ServeMux] pattern that matched the request.
	// It is empty if the request was not matched against a pattern.
	Pattern string
	// contains filtered or unexported fields
}
```

A Request represents an HTTP request received by a server or to be sent by a client.

The field semantics differ slightly between client and server usage. In addition to the notes on the fields below, see the documentation for [Request.Write](#Request.Write) and [RoundTripper](#RoundTripper).

#### func [NewRequest](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=872)

```
func NewRequest(method, url string, body io.Reader) (*Request, error)
```

NewRequest wraps [NewRequestWithContext](#NewRequestWithContext) using [context.Background](https://pkg.go.dev/context#Background).

#### func [NewRequestWithContext](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=899) added in go1.13

```
func NewRequestWithContext(ctx context.Context, method, url string, body io.Reader) (*Request, error)
```

NewRequestWithContext returns a new [Request](#Request) given a method, URL, and optional body.

If the provided body is also an [io.Closer](https://pkg.go.dev/io#Closer), the returned [Request.Body](#Request.Body) is set to body and will be closed (possibly asynchronously) by the Client methods Do, Post, and PostForm, and [Transport.RoundTrip](#Transport.RoundTrip).

NewRequestWithContext returns a Request suitable for use with [Client.Do](#Client.Do) or [Transport.RoundTrip](#Transport.RoundTrip). To create a request for use with testing a Server Handler, either use the [net/http/httptest.NewRequest](https://pkg.go.dev/net/http/httptest#NewRequest) function, use [ReadRequest](#ReadRequest), or manually update the Request fields. For an outgoing client request, the context controls the entire lifetime of a request and its response: obtaining a connection, sending the request, and reading the response headers and body. See the [Request](#Request) type's documentation for the difference between inbound and outbound request fields.

If body is of type [\*bytes.Buffer](https://pkg.go.dev/bytes#Buffer), [\*bytes.Reader](https://pkg.go.dev/bytes#Reader), or [\*strings.Reader](https://pkg.go.dev/strings#Reader), the returned request's ContentLength is set to its exact value (instead of -1), GetBody is populated (so 307 and 308 redirects can replay the body), and Body is set to [NoBody](#NoBody) if the ContentLength is 0.

#### func [ReadRequest](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1068)

```
func ReadRequest(b *bufio.Reader) (*Request, error)
```

ReadRequest reads and parses an incoming request from b.

ReadRequest is a low-level function and should only be used for specialized applications; most code should use the [Server](#Server) to read requests and handle them via the [Handler](#Handler) interface. ReadRequest only supports HTTP/1.x requests. For HTTP/2, use golang.org/x/net/http2.

#### func (\*Request) [AddCookie](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=469)

```
func (r *Request) AddCookie(c *Cookie)
```

AddCookie adds a cookie to the request. Per [RFC 6265 section 5.4](https://rfc-editor.org/rfc/rfc6265.html#section-5.4), AddCookie does not attach more than one [Cookie](#Cookie) header field. That means all cookies, if any, are written into the same line, separated by semicolon. AddCookie only sanitizes c's name and value, and does not sanitize a Cookie header already present in the request.

#### func (\*Request) [BasicAuth](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=983) added in go1.4

```
func (r *Request) BasicAuth() (username, password string, ok bool)
```

BasicAuth returns the username and password provided in the request's Authorization header, if the request uses HTTP Basic Authentication. See [RFC 2617, Section 2](https://rfc-editor.org/rfc/rfc2617.html#section-2).

#### func (\*Request) [Clone](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=391) added in go1.13

```
func (r *Request) Clone(ctx context.Context) *Request
```

Clone returns a deep copy of r with its context changed to ctx. The provided ctx must be non-nil.

Clone only makes a shallow copy of the Body field.

For an outgoing client request, the context controls the entire lifetime of a request and its response: obtaining a connection, sending the request, and reading the response headers and body.

#### func (\*Request) [Context](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=357) added in go1.7

```
func (r *Request) Context() context.Context
```

Context returns the request's context. To change the context, use [Request.Clone](#Request.Clone) or [Request.WithContext](#Request.WithContext).

The returned context is always non-nil; it defaults to the background context.

For outgoing client requests, the context controls cancellation.

For incoming server requests, the context is canceled when the client's connection closes, the request is canceled (with HTTP/2), or when the ServeHTTP method returns.

#### func (\*Request) [Cookie](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=453)

```
func (r *Request) Cookie(name string) (*Cookie, error)
```

Cookie returns the named cookie provided in the request or [ErrNoCookie](#ErrNoCookie) if not found. If multiple cookies match the given name, only one cookie will be returned.

#### func (\*Request) [Cookies](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=433)

```
func (r *Request) Cookies() []*Cookie
```

Cookies parses and returns the HTTP cookies sent with the request.

#### func (\*Request) [CookiesNamed](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=439) added in go1.23.0

```
func (r *Request) CookiesNamed(name string) []*Cookie
```

CookiesNamed parses and returns the named HTTP cookies sent with the request or an empty slice if none matched.

#### func (\*Request) [FormFile](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1469)

```
func (r *Request) FormFile(key string) (multipart.File, *multipart.FileHeader, error)
```

FormFile returns the first file for the provided form key. FormFile calls [Request.ParseMultipartForm](#Request.ParseMultipartForm) and [Request.ParseForm](#Request.ParseForm) if necessary.

#### func (\*Request) [FormValue](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1442)

```
func (r *Request) FormValue(key string) string
```

FormValue returns the first value for the named component of the query. The precedence order:

1. application/x-www-form-urlencoded form body (POST, PUT, PATCH only)
1. query parameters (always)
1. multipart/form-data form body (always)

FormValue calls [Request.ParseMultipartForm](#Request.ParseMultipartForm) and [Request.ParseForm](#Request.ParseForm) if necessary and ignores any errors returned by these functions. If key is not present, FormValue returns the empty string. To access multiple values of the same key, call ParseForm and then inspect [Request.Form](#Request.Form) directly.

#### func (\*Request) [MultipartReader](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=502)

```
func (r *Request) MultipartReader() (*multipart.Reader, error)
```

MultipartReader returns a MIME multipart reader if this is a multipart/form-data or a multipart/mixed POST request, else returns nil and an error. Use this function instead of [Request.ParseMultipartForm](#Request.ParseMultipartForm) to process the request body as a stream.

#### func (\*Request) [ParseForm](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1350)

```
func (r *Request) ParseForm() error
```

ParseForm populates r.Form and r.PostForm.

For all requests, ParseForm parses the raw query from the URL and updates r.Form.

For POST, PUT, and PATCH requests, it also reads the request body, parses it as a form and puts the results into both r.PostForm and r.Form. Request body parameters take precedence over URL query string values in r.Form.

If the request Body's size has not already been limited by [MaxBytesReader](#MaxBytesReader), the size is capped at 10MB.

For other HTTP methods, or when the Content-Type is not application/x-www-form-urlencoded, the request Body is not read, and r.PostForm is initialized to a non-nil, empty value.

[Request.ParseMultipartForm](#Request.ParseMultipartForm) calls ParseForm automatically. ParseForm is idempotent.

#### func (\*Request) [ParseMultipartForm](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1393)

```
func (r *Request) ParseMultipartForm(maxMemory int64) error
```

ParseMultipartForm parses a request body as multipart/form-data. The whole request body is parsed and up to a total of maxMemory bytes of its file parts are stored in memory, with the remainder stored on disk in temporary files. ParseMultipartForm calls [Request.ParseForm](#Request.ParseForm) if necessary. If ParseForm returns an error, ParseMultipartForm returns it but also continues parsing the request body. After one call to ParseMultipartForm, subsequent calls have no effect.

#### func (\*Request) [PathValue](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1495) added in go1.22.0

```
func (r *Request) PathValue(name string) string
```

PathValue returns the value for the named path wildcard in the [ServeMux](#ServeMux) pattern that matched the request. It returns the empty string if the request was not matched against a pattern or there is no such wildcard in the pattern.

The value is unescaped. For example, if the pattern "/b/{bucket}" matches the path "/b/a%2fb", PathValue("bucket") returns "a/b".

#### func (\*Request) [PostFormValue](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1457) added in go1.1

```
func (r *Request) PostFormValue(key string) string
```

PostFormValue returns the first value for the named component of the POST, PUT, or PATCH request body. URL query parameters are ignored. PostFormValue calls [Request.ParseMultipartForm](#Request.ParseMultipartForm) and [Request.ParseForm](#Request.ParseForm) if necessary and ignores any errors returned by these functions. If key is not present, PostFormValue returns the empty string.

#### func (\*Request) [ProtoAtLeast](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=422)

```
func (r *Request) ProtoAtLeast(major, minor int) bool
```

ProtoAtLeast reports whether the HTTP protocol used in the request is at least major.minor.

#### func (\*Request) [Referer](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=486)

```
func (r *Request) Referer() string
```

Referer returns the referring URL, if sent in the request.

Referer is misspelled as in the request itself, a mistake from the earliest days of HTTP. This value can also be fetched from the [Header](#Header) map as Header\["Referer"\]; the benefit of making it available as a method is that the compiler can diagnose programs that use the alternate (correct English) spelling req.Referrer() but cannot diagnose programs that use Header\["Referrer"\].

#### func (\*Request) [SetBasicAuth](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1032)

```
func (r *Request) SetBasicAuth(username, password string)
```

SetBasicAuth sets the request's Authorization header to use HTTP Basic Authentication with the provided username and password.

With HTTP Basic Authentication the provided username and password are not encrypted. It should generally only be used in an HTTPS request.

The username may not contain a colon. Some protocols may impose additional requirements on pre-escaping the username and password. For instance, when used with OAuth2, both arguments must be URL encoded first with [url.QueryEscape](https://pkg.go.dev/net/url#QueryEscape).

#### func (\*Request) [SetPathValue](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=1505) added in go1.22.0

```
func (r *Request) SetPathValue(name, value string)
```

SetPathValue sets name to value, so that subsequent calls to r.PathValue(name) return value. It does not unescape value.

#### func (\*Request) [UserAgent](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=428)

```
func (r *Request) UserAgent() string
```

UserAgent returns the client's User-Agent, if sent in the request.

#### func (\*Request) [WithContext](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=373) added in go1.7

```
func (r *Request) WithContext(ctx context.Context) *Request
```

WithContext returns a shallow copy of r with its context changed to ctx. The provided ctx must be non-nil.

For outgoing client request, the context controls the entire lifetime of a request and its response: obtaining a connection, sending the request, and reading the response headers and body.

To create a new request with a context, use [NewRequestWithContext](#NewRequestWithContext). To make a deep copy of a request with a new context, use [Request.Clone](#Request.Clone).

#### func (\*Request) [Write](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=571)

```
func (r *Request) Write(w io.Writer) error
```

Write writes an HTTP/1.1 request, which is the header and body, in wire format. This method consults the following fields of the request:

```
Host
URL
Method (defaults to "GET")
Header
ContentLength
TransferEncoding
Body
```

If Body is present, Content-Length is \<= 0 and [Request.TransferEncoding](#Request.TransferEncoding) hasn't been set to "identity", Write adds "Transfer-Encoding: chunked" to the header. Body is closed after it is sent.

Header values for Host, Content-Length, Transfer-Encoding, and Trailer are not used; these are derived from other Request fields. If the Header does not contain a User-Agent value, Write uses "Go-http-client/1.1".

#### func (\*Request) [WriteProxy](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/request.go;l=581)

```
func (r *Request) WriteProxy(w io.Writer) error
```

WriteProxy is like [Request.Write](#Request.Write) but writes the request in the form expected by an HTTP proxy. In particular, [Request.WriteProxy](#Request.WriteProxy) writes the initial Request-URI line of the request with an absolute URI, per section 5.3 of [RFC 7230](https://rfc-editor.org/rfc/rfc7230.html), including the scheme and host. In either case, WriteProxy also writes a Host header, using either r.Host or r.URL.Host.


#### type [Response](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/response.go;l=36)

```
type Response struct {
	Status     string // e.g. "200 OK"
	StatusCode int    // e.g. 200
	Proto      string // e.g. "HTTP/1.0"
	ProtoMajor int    // e.g. 1
	ProtoMinor int    // e.g. 0

	// Header maps header keys to values. If the response had multiple
	// headers with the same key, they may be concatenated, with comma
	// delimiters.  (RFC 7230, section 3.2.2 requires that multiple headers
	// be semantically equivalent to a comma-delimited sequence.) When
	// Header values are duplicated by other fields in this struct (e.g.,
	// ContentLength, TransferEncoding, Trailer), the field values are
	// authoritative.
	//
	// Keys in the map are canonicalized (see CanonicalHeaderKey).
	Header Header

	// Body represents the response body.
	//
	// The response body is streamed on demand as the Body field
	// is read. If the network connection fails or the server
	// terminates the response, Body.Read calls return an error.
	//
	// The http Client and Transport guarantee that Body is always
	// non-nil, even on responses without a body or responses with
	// a zero-length body. It is the caller's responsibility to
	// close Body. The default HTTP client's Transport may not
	// reuse HTTP/1.x "keep-alive" TCP connections if the Body is
	// not read to completion and closed; however, manually reading
	// the body to completion should not be needed in most cases,
	// as closing the body will also cause the body to be read to
	// completion asynchronously, up to a conservative limit.
	//
	// The Body is automatically dechunked if the server replied
	// with a "chunked" Transfer-Encoding.
	//
	// As of Go 1.12, the Body will also implement io.Writer
	// on a successful "101 Switching Protocols" response,
	// as used by WebSockets and HTTP/2's "h2c" mode.
	Body io.ReadCloser

	// ContentLength records the length of the associated content. The
	// value -1 indicates that the length is unknown. Unless Request.Method
	// is "HEAD", values >= 0 indicate that the given number of bytes may
	// be read from Body.
	ContentLength int64

	// Contains transfer encodings from outer-most to inner-most. Value is
	// nil, means that "identity" encoding is used.
	TransferEncoding []string

	// Close records whether the header directed that the connection be
	// closed after reading Body. The value is advice for clients: neither
	// ReadResponse nor Response.Write ever closes a connection.
	Close bool

	// Uncompressed reports whether the response was sent compressed but
	// was decompressed by the http package. When true, reading from
	// Body yields the uncompressed content instead of the compressed
	// content actually set from the server, ContentLength is set to -1,
	// and the "Content-Length" and "Content-Encoding" fields are deleted
	// from the responseHeader. To get the original response from
	// the server, set Transport.DisableCompression to true.
	Uncompressed bool

	// Trailer maps trailer keys to values in the same
	// format as Header.
	//
	// The Trailer initially contains only nil values, one for
	// each key specified in the server's "Trailer" header
	// value. Those values are not added to Header.
	//
	// Trailer must not be accessed concurrently with Read calls
	// on the Body.
	//
	// After Body.Read has returned io.EOF, Trailer will contain
	// any trailer values sent by the server.
	Trailer Header

	// Request is the request that was sent to obtain this Response.
	// Request's Body is nil (having already been consumed).
	// This is only populated for Client requests.
	Request *Request

	// TLS contains information about the TLS connection on which the
	// response was received. It is nil for unencrypted responses.
	// The pointer is shared between responses and should not be
	// modified.
	TLS *tls.ConnectionState
}
```

Response represents the response from an HTTP request.

The [Client](#Client) and [Transport](#Transport) return Responses from servers once the response headers have been received. The response body is streamed on demand as the Body field is read.

#### func [Get](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=459)

```
func Get(url string) (resp *Response, err error)
```

Get issues a GET to the specified URL. If the response is one of the following redirect codes, Get follows the redirect, up to a maximum of 10 redirects:

```
301 (Moved Permanently)
302 (Found)
303 (See Other)
307 (Temporary Redirect)
308 (Permanent Redirect)
```

An error is returned if there were too many redirects or if there was an HTTP protocol error. A non-2xx response doesn't cause an error. Any returned error will be of type [\*url.Error](https://pkg.go.dev/net/url#Error). The url.Error value's Timeout method will report true if the request timed out.

When err is nil, resp always contains a non-nil resp.Body. Caller should close resp.Body when done reading from it.

Get is a wrapper around DefaultClient.Get.

To make a request with custom headers, use [NewRequest](#NewRequest) and DefaultClient.Do.

To make a request with a specified context.Context, use [NewRequestWithContext](#NewRequestWithContext) and DefaultClient.Do.

Example [¶](#example-Get "Go to Example")

```
package main

import (
	"fmt"
	"io"
	"log"
	"net/http"
)

func main() {
	res, err := http.Get("http://www.google.com/robots.txt")
	if err != nil {
		log.Fatal(err)
	}
	body, err := io.ReadAll(res.Body)
	res.Body.Close()
	if res.StatusCode > 299 {
		log.Fatalf("Response failed with status code: %d and\nbody: %s\n", res.StatusCode, body)
	}
	if err != nil {
		log.Fatal(err)
	}
	fmt.Printf("%s", body)
}
```



#### func [Head](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=947)

```
func Head(url string) (resp *Response, err error)
```

Head issues a HEAD to the specified URL. If the response is one of the following redirect codes, Head follows the redirect, up to a maximum of 10 redirects:

```
301 (Moved Permanently)
302 (Found)
303 (See Other)
307 (Temporary Redirect)
308 (Permanent Redirect)
```

Head is a wrapper around DefaultClient.Head.

To make a request with a specified [context.Context](https://pkg.go.dev/context#Context), use [NewRequestWithContext](#NewRequestWithContext) and DefaultClient.Do.

#### func [Post](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=868)

```
func Post(url, contentType string, body io.Reader) (resp *Response, err error)
```

Post issues a POST to the specified URL.

Caller should close resp.Body when done reading from it.

If the provided body is an [io.Closer](https://pkg.go.dev/io#Closer), it is closed after the request.

Post is a wrapper around DefaultClient.Post.

To set custom headers, use [NewRequest](#NewRequest) and DefaultClient.Do.

See the [Client.Do](#Client.Do) method documentation for details on how redirects are handled.

To make a request with a specified context.Context, use [NewRequestWithContext](#NewRequestWithContext) and DefaultClient.Do.

#### func [PostForm](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=911)

```
func PostForm(url string, data url.Values) (resp *Response, err error)
```

PostForm issues a POST to the specified URL, with data's keys and values URL-encoded as the request body.

The Content-Type header is set to application/x-www-form-urlencoded. To set other headers, use [NewRequest](#NewRequest) and DefaultClient.Do.

When err is nil, resp always contains a non-nil resp.Body. Caller should close resp.Body when done reading from it.

PostForm is a wrapper around DefaultClient.PostForm.

See the [Client.Do](#Client.Do) method documentation for details on how redirects are handled.

To make a request with a specified [context.Context](https://pkg.go.dev/context#Context), use [NewRequestWithContext](#NewRequestWithContext) and DefaultClient.Do.

#### func [ReadResponse](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/response.go;l=158)

```
func ReadResponse(r *bufio.Reader, req *Request) (*Response, error)
```

ReadResponse reads and returns an HTTP response from r. The req parameter optionally specifies the [Request](#Request) that corresponds to this [Response](#Response). If nil, a GET request is assumed. Clients must call resp.Body.Close when finished reading resp.Body. After that call, clients can inspect resp.Trailer to find key/value pairs included in the response trailer.

#### func (\*Response) [Cookies](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/response.go;l=129)

```
func (r *Response) Cookies() []*Cookie
```

Cookies parses and returns the cookies set in the Set-Cookie headers.

#### func (\*Response) [Location](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/response.go;l=141)

```
func (r *Response) Location() (*url.URL, error)
```

Location returns the URL of the response's "Location" header, if present. Relative redirects are resolved relative to [Response.Request](#Response.Request). [ErrNoLocation](#ErrNoLocation) is returned if no Location header is present.

#### func (\*Response) [ProtoAtLeast](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/response.go;l=228)

```
func (r *Response) ProtoAtLeast(major, minor int) bool
```

ProtoAtLeast reports whether the HTTP protocol used in the response is at least major.minor.

#### func (\*Response) [Write](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/response.go;l=249)

```
func (r *Response) Write(w io.Writer) error
```

Write writes r to w in the HTTP/1.x server response format, including the status line, headers, body, and optional trailer.

This method consults the following fields of the response r:

```
StatusCode
ProtoMajor
ProtoMinor
Request.Method
TransferEncoding
Trailer
Body
ContentLength
Header, values for non-canonical keys will have unpredictable behavior
```

The Response Body is closed after it is sent.


#### type [ResponseController](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/responsecontroller.go;l=17) added in go1.20

```
type ResponseController struct {
	// contains filtered or unexported fields
}
```

A ResponseController is used by an HTTP handler to control the response.

A ResponseController may not be used after the [Handler.ServeHTTP](#Handler.ServeHTTP) method has returned.

#### func [NewResponseController](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/responsecontroller.go;l=38) added in go1.20

```
func NewResponseController(rw ResponseWriter) *ResponseController
```

NewResponseController creates a [ResponseController](#ResponseController) for a request.

The ResponseWriter should be the original value passed to the [Handler.ServeHTTP](#Handler.ServeHTTP) method, or have an Unwrap method returning the original ResponseWriter.

If the ResponseWriter implements any of the following methods, the ResponseController will call them as appropriate:

```
Flush()
FlushError() error // alternative Flush returning an error
Hijack() (net.Conn, *bufio.ReadWriter, error)
SetReadDeadline(deadline time.Time) error
SetWriteDeadline(deadline time.Time) error
EnableFullDuplex() error
```

If the ResponseWriter does not support a method, ResponseController returns an error matching [ErrNotSupported](#ErrNotSupported).

#### func (\*ResponseController) [EnableFullDuplex](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/responsecontroller.go;l=129) added in go1.21.0

```
func (c *ResponseController) EnableFullDuplex() error
```

EnableFullDuplex indicates that the request handler will interleave reads from [Request.Body](#Request.Body) with writes to the [ResponseWriter](#ResponseWriter).

For HTTP/1 requests, the Go HTTP server by default consumes any unread portion of the request body before beginning to write the response, preventing handlers from concurrently reading from the request and writing the response. Calling EnableFullDuplex disables this behavior and permits handlers to continue to read from the request while concurrently writing the response.

For HTTP/2 requests, the Go HTTP server always permits concurrent reads and responses.

#### func (\*ResponseController) [Flush](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/responsecontroller.go;l=47) added in go1.20

```
func (c *ResponseController) Flush() error
```

Flush flushes buffered data to the client.

#### func (\*ResponseController) [Hijack](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/responsecontroller.go;l=66) added in go1.20

```
func (c *ResponseController) Hijack() (net.Conn, *bufio.ReadWriter, error)
```

Hijack lets the caller take over the connection. See the [Hijacker](#Hijacker) interface for details.

#### func (\*ResponseController) [SetReadDeadline](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/responsecontroller.go;l=85) added in go1.20

```
func (c *ResponseController) SetReadDeadline(deadline time.Time) error
```

SetReadDeadline sets the deadline for reading the entire request, including the body. Reads from the request body after the deadline has been exceeded will return an error. A zero value means no deadline.

Setting the read deadline after it has been exceeded will not extend it.

#### func (\*ResponseController) [SetWriteDeadline](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/responsecontroller.go;l=105) added in go1.20

```
func (c *ResponseController) SetWriteDeadline(deadline time.Time) error
```

SetWriteDeadline sets the deadline for writing the response. Writes to the response body after the deadline has been exceeded will not block, but may succeed if the data has been buffered. A zero value means no deadline.

Setting the write deadline after it has been exceeded will not extend it.


#### type [ResponseWriter](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=97)

```
type ResponseWriter interface {
	// Header returns the header map that will be sent by
	// [ResponseWriter.WriteHeader]. The [Header] map also is the mechanism with which
	// [Handler] implementations can set HTTP trailers.
	//
	// Changing the header map after a call to [ResponseWriter.WriteHeader] (or
	// [ResponseWriter.Write]) has no effect unless the HTTP status code was of the
	// 1xx class or the modified headers are trailers.
	//
	// There are two ways to set Trailers. The preferred way is to
	// predeclare in the headers which trailers you will later
	// send by setting the "Trailer" header to the names of the
	// trailer keys which will come later. In this case, those
	// keys of the Header map are treated as if they were
	// trailers. See the example. The second way, for trailer
	// keys not known to the [Handler] until after the first [ResponseWriter.Write],
	// is to prefix the [Header] map keys with the [TrailerPrefix]
	// constant value.
	//
	// To suppress automatic response headers (such as "Date"), set
	// their value to nil.
	Header() Header

	// Write writes the data to the connection as part of an HTTP reply.
	//
	// If [ResponseWriter.WriteHeader] has not yet been called, Write calls
	// WriteHeader(http.StatusOK) before writing the data. If the Header
	// does not contain a Content-Type line, Write adds a Content-Type set
	// to the result of passing the initial 512 bytes of written data to
	// [DetectContentType]. Additionally, if the total size of all written
	// data is under a few KB and there are no Flush calls, the
	// Content-Length header is added automatically.
	//
	// Depending on the HTTP protocol version and the client, calling
	// Write or WriteHeader may prevent future reads on the
	// Request.Body. For HTTP/1.x requests, handlers should read any
	// needed request body data before writing the response. Once the
	// headers have been flushed (due to either an explicit Flusher.Flush
	// call or writing enough data to trigger a flush), the request body
	// may be unavailable. For HTTP/2 requests, the Go HTTP server permits
	// handlers to continue to read the request body while concurrently
	// writing the response. However, such behavior may not be supported
	// by all HTTP/2 clients. Handlers should read before writing if
	// possible to maximize compatibility.
	Write([]byte) (int, error)

	// WriteHeader sends an HTTP response header with the provided
	// status code.
	//
	// If WriteHeader is not called explicitly, the first call to Write
	// will trigger an implicit WriteHeader(http.StatusOK).
	// Thus explicit calls to WriteHeader are mainly used to
	// send error codes or 1xx informational responses.
	//
	// The provided code must be a valid HTTP 1xx-5xx status code.
	// Any number of 1xx headers may be written, followed by at most
	// one 2xx-5xx header. 1xx headers are sent immediately, but 2xx-5xx
	// headers may be buffered. Use the Flusher interface to send
	// buffered data. The header map is cleared when 2xx-5xx headers are
	// sent, but not with 1xx headers.
	//
	// The server will automatically send a 100 (Continue) header
	// on the first read from the request body if the request has
	// an "Expect: 100-continue" header.
	WriteHeader(statusCode int)
}
```

A ResponseWriter interface is used by an HTTP handler to construct an HTTP response.

A ResponseWriter may not be used after [Handler.ServeHTTP](#Handler.ServeHTTP) has returned.

Example (Trailers) [¶](#example-ResponseWriter-Trailers "Go to Example (Trailers)")

HTTP Trailers are a set of key/value pairs like headers that come after the HTTP response, instead of before.

```
package main

import (
	"io"
	"net/http"
)

func main() {
	mux := http.NewServeMux()
	mux.HandleFunc("/sendstrailers", func(w http.ResponseWriter, req *http.Request) {
		// Before any call to WriteHeader or Write, declare
		// the trailers you will set during the HTTP
		// response. These three headers are actually sent in
		// the trailer.
		w.Header().Set("Trailer", "AtEnd1, AtEnd2")
		w.Header().Add("Trailer", "AtEnd3")

		w.Header().Set("Content-Type", "text/plain; charset=utf-8") // normal header
		w.WriteHeader(http.StatusOK)

		w.Header().Set("AtEnd1", "value 1")
		io.WriteString(w, "This HTTP response has both headers before this text and trailers at the end.\n")
		w.Header().Set("AtEnd2", "value 2")
		w.Header().Set("AtEnd3", "value 3") // These will appear as trailers.
	})
}
```



#### type [RoundTripper](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/client.go;l=118)

```
type RoundTripper interface {
	// RoundTrip executes a single HTTP transaction, returning
	// a Response for the provided Request.
	//
	// RoundTrip should not attempt to interpret the response. In
	// particular, RoundTrip must return err == nil if it obtained
	// a response, regardless of the response's HTTP status code.
	// A non-nil err should be reserved for failure to obtain a
	// response. Similarly, RoundTrip should not attempt to
	// handle higher-level protocol details such as redirects,
	// authentication, or cookies.
	//
	// RoundTrip should not modify the request, except for
	// consuming and closing the Request's Body. RoundTrip may
	// read fields of the request in a separate goroutine. Callers
	// should not mutate or reuse the request until the Response's
	// Body has been closed.
	//
	// RoundTrip must always close the body, including on errors,
	// but depending on the implementation may do so in a separate
	// goroutine even after RoundTrip returns. This means that
	// callers wanting to reuse the body for subsequent requests
	// must arrange to wait for the Close call before doing so.
	//
	// The Request's URL and Header fields must be initialized.
	RoundTrip(*Request) (*Response, error)
}
```

RoundTripper is an interface representing the ability to execute a single HTTP transaction, obtaining the [Response](#Response) for a given [Request](#Request).

A RoundTripper must be safe for concurrent use by multiple goroutines.

```
var DefaultTransport RoundTripper = &Transport{
	Proxy: ProxyFromEnvironment,
	DialContext: defaultTransportDialContext(&net.Dialer{
		Timeout:   30 * time.Second,
		KeepAlive: 30 * time.Second,
	}),
	ForceAttemptHTTP2:     true,
	MaxIdleConns:          100,
	IdleConnTimeout:       90 * time.Second,
	TLSHandshakeTimeout:   10 * time.Second,
	ExpectContinueTimeout: 1 * time.Second,
}
```

DefaultTransport is the default implementation of [Transport](#Transport) and is used by [DefaultClient](#DefaultClient). It establishes network connections as needed and caches them for reuse by subsequent calls. It uses HTTP proxies as directed by the environment variables HTTP\_PROXY, HTTPS\_PROXY and NO\_PROXY (or the lowercase versions thereof).

#### func [NewFileTransport](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/filetransport.go;l=31)

```
func NewFileTransport(fs FileSystem) RoundTripper
```

NewFileTransport returns a new [RoundTripper](#RoundTripper), serving the provided [FileSystem](#FileSystem). The returned RoundTripper ignores the URL host in its incoming requests, as well as most other properties of the request.

The typical use case for NewFileTransport is to register the "file" protocol with a [Transport](#Transport), as in:

```
t := &http.Transport{}
t.RegisterProtocol("file", http.NewFileTransport(http.Dir("/")))
c := &http.Client{Transport: t}
res, err := c.Get("file:///etc/passwd")
...
```

#### func [NewFileTransportFS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/filetransport.go;l=49) added in go1.22.0

```
func NewFileTransportFS(fsys fs.FS) RoundTripper
```

NewFileTransportFS returns a new [RoundTripper](#RoundTripper), serving the provided file system fsys. The returned RoundTripper ignores the URL host in its incoming requests, as well as most other properties of the request. The files provided by fsys must implement [io.Seeker](https://pkg.go.dev/io#Seeker).

The typical use case for NewFileTransportFS is to register the "file" protocol with a [Transport](#Transport), as in:

```
fsys := os.DirFS("/")
t := &http.Transport{}
t.RegisterProtocol("file", http.NewFileTransportFS(fsys))
c := &http.Client{Transport: t}
res, err := c.Get("file:///etc/passwd")
...
```


#### type [SameSite](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/cookie.go;l=54) added in go1.11

```
type SameSite int
```

SameSite allows a server to define a cookie attribute making it impossible for the browser to send this cookie along with cross-site requests. The main goal is to mitigate the risk of cross-origin information leakage, and provide some protection against cross-site request forgery attacks.

See [https://tools.ietf.org/html/draft-ietf-httpbis-cookie-same-site-00](https://tools.ietf.org/html/draft-ietf-httpbis-cookie-same-site-00) for details.

```
const (
	SameSiteDefaultMode SameSite = iota + 1
	SameSiteLaxMode
	SameSiteStrictMode
	SameSiteNoneMode
)
```

#### type [ServeMux](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2627)

```
type ServeMux struct {
	// contains filtered or unexported fields
}
```

- [Patterns](#hdr-Patterns-ServeMux)
- [Precedence](#hdr-Precedence-ServeMux)
- [Trailing-slash redirection](#hdr-Trailing_slash_redirection-ServeMux)
- [Request sanitizing](#hdr-Request_sanitizing-ServeMux)
- [Compatibility](#hdr-Compatibility-ServeMux)

ServeMux is an HTTP request multiplexer. It matches the URL of each incoming request against a list of registered patterns and calls the handler for the pattern that most closely matches the URL.

#### Patterns

Patterns can match the method, host and path of a request. Some examples:

- "/index.html" matches the path "/index.html" for any host and method.
- "GET /static/" matches a GET request whose path begins with "/static/".
- "example.com/" matches any request to the host "example.com".
- "example.com/{$}" matches requests with host "example.com" and path "/".
- "/b/{bucket}/o/{objectname...}" matches paths whose first segment is "b" and whose third segment is "o". The name "bucket" denotes the second segment and "objectname" denotes the remainder of the path.

In general, a pattern looks like

```
[METHOD ][HOST]/[PATH]
```

All three parts are optional; "/" is a valid pattern. If METHOD is present, it must be followed by at least one space or tab.

Literal (that is, non-wildcard) parts of a pattern match the corresponding parts of a request case-sensitively.

A pattern with no method matches every method. A pattern with the method GET matches both GET and HEAD requests. Otherwise, the method must match exactly.

A pattern with no host matches every host. A pattern with a host matches URLs on that host only.

A path can include wildcard segments of the form {NAME} or {NAME...}. For example, "/b/{bucket}/o/{objectname...}". The wildcard name must be a valid Go identifier. Wildcards must be full path segments: they must be preceded by a slash and followed by either a slash or the end of the string. For example, "/b\_{bucket}" is not a valid pattern.

Normally a wildcard matches only a single path segment, ending at the next literal slash (not %2F) in the request URL. But if the "..." is present, then the wildcard matches the remainder of the URL path, including slashes. (Therefore it is invalid for a "..." wildcard to appear anywhere but at the end of a pattern.) The match for a wildcard can be obtained by calling [Request.PathValue](#Request.PathValue) with the wildcard's name. A trailing slash in a path acts as an anonymous "..." wildcard.

The special wildcard {$} matches only the end of the URL. For example, the pattern "/{$}" matches only the path "/", whereas the pattern "/" matches every path.

For matching, both pattern paths and incoming request paths are unescaped segment by segment. So, for example, the path "/a%2Fb/100%25" is treated as having two segments, "a/b" and "100%". The pattern "/a%2fb/" matches it, but the pattern "/a/b/" does not.

#### Precedence

If two or more patterns match a request, then the most specific pattern takes precedence. A pattern P1 is more specific than P2 if P1 matches a strict subset of P2’s requests; that is, if P2 matches all the requests of P1 and more. If neither is more specific, then the patterns conflict. There is one exception to this rule, for backwards compatibility: if two patterns would otherwise conflict and one has a host while the other does not, then the pattern with the host takes precedence. If a pattern passed to [ServeMux.Handle](#ServeMux.Handle) or [ServeMux.HandleFunc](#ServeMux.HandleFunc) conflicts with another pattern that is already registered, those functions panic.

As an example of the general rule, "/images/thumbnails/" is more specific than "/images/", so both can be registered. The former matches paths beginning with "/images/thumbnails/" and the latter will match any other path in the "/images/" subtree.

As another example, consider the patterns "GET /" and "/index.html": both match a GET request for "/index.html", but the former pattern matches all other GET and HEAD requests, while the latter matches any request for "/index.html" that uses a different method. The patterns conflict.

#### Trailing-slash redirection

Consider a [ServeMux](#ServeMux) with a handler for a subtree, registered using a trailing slash or "..." wildcard. If the ServeMux receives a request for the subtree root without a trailing slash, it redirects the request by adding the trailing slash. This behavior can be overridden with a separate registration for the path without the trailing slash or "..." wildcard. For example, registering "/images/" causes ServeMux to redirect a request for "/images" to "/images/", unless "/images" has been registered separately.

#### Request sanitizing

ServeMux also takes care of sanitizing the URL request path and the Host header, stripping the port number and redirecting any request containing . or .. segments or repeated slashes to an equivalent, cleaner URL. Escaped path elements such as "%2e" for "." and "%2f" for "/" are preserved and aren't considered separators for request routing.

#### Compatibility

The pattern syntax and matching behavior of ServeMux changed significantly in Go 1.22. To restore the old behavior, set the GODEBUG environment variable to "httpmuxgo121=1". This setting is read once, at program startup; changes during execution will be ignored.

The backwards-incompatible changes include:

- Wildcards are just ordinary literal path segments in 1.21. For example, the pattern "/{x}" will match only that path in 1.21, but will match any one-segment path in 1.22.
- In 1.21, no pattern was rejected, unless it was empty or conflicted with an existing pattern. In 1.22, syntactically invalid patterns will cause [ServeMux.Handle](#ServeMux.Handle) and [ServeMux.HandleFunc](#ServeMux.HandleFunc) to panic. For example, in 1.21, the patterns "/{" and "/a{x}" match themselves, but in 1.22 they are invalid and will cause a panic when registered.
- In 1.22, each segment of a pattern is unescaped; this was not done in 1.21. For example, in 1.22 the pattern "/%61" matches the path "/a" ("%61" being the URL escape sequence for "a"), but in 1.21 it would match only the path "/%2561" (where "%25" is the escape for the percent sign).
- When matching patterns to paths, in 1.22 each segment of the path is unescaped; in 1.21, the entire path is unescaped. This change mostly affects how paths with %2F escapes adjacent to slashes are treated. See [https://go.dev/issue/21955](https://go.dev/issue/21955) for details.

#### func [NewServeMux](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2635)

```
func NewServeMux() *ServeMux
```

NewServeMux allocates and returns a new [ServeMux](#ServeMux).

#### func (\*ServeMux) [Handle](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2914)

```
func (mux *ServeMux) Handle(pattern string, handler Handler)
```

Handle registers the handler for the given pattern. If the given pattern conflicts with one that is already registered or if the pattern is invalid, Handle panics.

See [ServeMux](#ServeMux) for details on valid patterns and conflict rules.

Example [¶](#example-ServeMux.Handle "Go to Example")

```
package main

import (
	"fmt"
	"net/http"
)

type apiHandler struct{}

func (apiHandler) ServeHTTP(http.ResponseWriter, *http.Request) {}

func main() {
	mux := http.NewServeMux()
	mux.Handle("/api/", apiHandler{})
	mux.HandleFunc("/", func(w http.ResponseWriter, req *http.Request) {
		// The "/" pattern matches everything, so we need to check
		// that we're at the root here.
		if req.URL.Path != "/" {
			http.NotFound(w, req)
			return
		}
		fmt.Fprintf(w, "Welcome to the home page!")
	})
}
```



#### func (\*ServeMux) [HandleFunc](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2927)

```
func (mux *ServeMux) HandleFunc(pattern string, handler func(ResponseWriter, *Request))
```

HandleFunc registers the handler function for the given pattern. If the given pattern conflicts with one that is already registered or if the pattern is invalid, HandleFunc panics.

See [ServeMux](#ServeMux) for details on valid patterns and conflict rules.

#### func (\*ServeMux) [Handler](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2699) added in go1.1

```
func (mux *ServeMux) Handler(r *Request) (h Handler, pattern string)
```

Handler returns the handler to use for the given request, consulting r.Method, r.Host, and r.URL.Path. It always returns a non-nil handler. If the path is not in its canonical form, the handler will be an internally-generated handler that redirects to the canonical path. If the host contains a port, it is ignored when matching handlers.

The path and host are used unchanged for CONNECT requests.

Handler also returns the registered pattern that matches the request or, in the case of internally-generated redirects, the path that will match after following the redirect.

If there is no registered handler that applies to the request, Handler returns a “page not found” or “method not supported” handler and an empty pattern.

Handler does not modify its argument. In particular, it does not populate named path wildcards, so r.PathValue will always return the empty string.

#### func (\*ServeMux) [ServeHTTP](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=2889)

```
func (mux *ServeMux) ServeHTTP(w ResponseWriter, r *Request)
```

ServeHTTP dispatches the request to the handler whose pattern most closely matches the request URL.


#### type [Server](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3040)

```
type Server struct {
	// Addr optionally specifies the TCP address for the server to listen on,
	// in the form "host:port". If empty, ":http" (port 80) is used.
	// The service names are defined in RFC 6335 and assigned by IANA.
	// See net.Dial for details of the address format.
	Addr string

	Handler Handler // handler to invoke, http.DefaultServeMux if nil

	// DisableGeneralOptionsHandler, if true, passes "OPTIONS *" requests to the Handler,
	// otherwise responds with 200 OK and Content-Length: 0.
	DisableGeneralOptionsHandler bool

	// TLSConfig optionally provides a TLS configuration for use
	// by ServeTLS and ListenAndServeTLS. Note that this value is
	// cloned by ServeTLS and ListenAndServeTLS, so it's not
	// possible to modify the configuration with methods like
	// tls.Config.SetSessionTicketKeys. To use
	// SetSessionTicketKeys, use Server.Serve with a TLS Listener
	// instead.
	TLSConfig *tls.Config

	// ReadTimeout is the maximum duration for reading the entire
	// request, including the body. A zero or negative value means
	// there will be no timeout.
	//
	// Because ReadTimeout does not let Handlers make per-request
	// decisions on each request body's acceptable deadline or
	// upload rate, most users will prefer to use
	// ReadHeaderTimeout. It is valid to use them both.
	ReadTimeout time.Duration

	// ReadHeaderTimeout is the amount of time allowed to read
	// request headers. The connection's read deadline is reset
	// after reading the headers and the Handler can decide what
	// is considered too slow for the body. If zero, the value of
	// ReadTimeout is used. If negative, or if zero and ReadTimeout
	// is zero or negative, there is no timeout.
	ReadHeaderTimeout time.Duration

	// WriteTimeout is the maximum duration before timing out
	// writes of the response. It is reset whenever a new
	// request's header is read. Like ReadTimeout, it does not
	// let Handlers make decisions on a per-request basis.
	// A zero or negative value means there will be no timeout.
	WriteTimeout time.Duration

	// IdleTimeout is the maximum amount of time to wait for the
	// next request when keep-alives are enabled. If zero, the value
	// of ReadTimeout is used. If negative, or if zero and ReadTimeout
	// is zero or negative, there is no timeout.
	IdleTimeout time.Duration

	// MaxHeaderBytes controls the maximum number of bytes the
	// server will read parsing the request header's keys and
	// values, including the request line. It does not limit the
	// size of the request body.
	// If zero, DefaultMaxHeaderBytes is used.
	MaxHeaderBytes int

	// MaxHeaderValueCount controls the maximum number of header
	// values that the server is willing to parse from a request.
	// If zero, DefaultMaxHeaderValueCount is used.
	// Note that comma-separated values in a single header line are
	// counted once, while values sent as multiple header lines are
	// counted multiple times.
	MaxHeaderValueCount int

	// TLSNextProto optionally specifies a function to take over
	// ownership of the provided TLS connection when an ALPN
	// protocol upgrade has occurred. The map key is the protocol
	// name negotiated. The Handler argument should be used to
	// handle HTTP requests and will initialize the Request's TLS
	// and RemoteAddr if not already set. The connection is
	// automatically closed when the function returns.
	// If TLSNextProto is not nil, HTTP/2 support is not enabled
	// automatically.
	//
	// Historically, TLSNextProto was used to disable HTTP/2 support.
	// The Server.Protocols field now provides a simpler way to do this.
	TLSNextProto map[string]func(*Server, *tls.Conn, Handler)

	// ConnState specifies an optional callback function that is
	// called when a client connection changes state. See the
	// ConnState type and associated constants for details.
	ConnState func(net.Conn, ConnState)

	// ErrorLog specifies an optional logger for errors accepting
	// connections, unexpected behavior from handlers, and
	// underlying FileSystem errors.
	// If nil, logging is done via the log package's standard logger.
	ErrorLog *log.Logger

	// BaseContext optionally specifies a function that returns
	// the base context for incoming requests on this server.
	// The provided Listener is the specific Listener that's
	// about to start accepting requests.
	// If BaseContext is nil, the default is context.Background().
	// If non-nil, it must return a non-nil context.
	BaseContext func(net.Listener) context.Context

	// ConnContext optionally specifies a function that modifies
	// the context used for a new connection c. The provided ctx
	// is derived from the base context and has a ServerContextKey
	// value.
	ConnContext func(ctx context.Context, c net.Conn) context.Context

	// HTTP2 configures HTTP/2 connections.
	HTTP2 *HTTP2Config

	// Protocols is the set of protocols accepted by the server.
	//
	// If Protocols includes UnencryptedHTTP2, the server will accept
	// unencrypted HTTP/2 connections. The server can serve both
	// HTTP/1 and unencrypted HTTP/2 on the same address and port.
	//
	// If Protocols is nil, the default is usually HTTP/1 and HTTP/2.
	// If TLSNextProto is non-nil and does not contain an "h2" entry,
	// the default is HTTP/1 only.
	Protocols *Protocols

	// DisableClientPriority specifies whether client-specified priority, as
	// specified in RFC 9218, should be respected or not.
	//
	// This field only takes effect if using HTTP/2, and if no custom write
	// scheduler is defined for the HTTP/2 server. Otherwise, this field is a
	// no-op.
	//
	// If set to true, requests will be served in a round-robin manner, without
	// prioritization.
	DisableClientPriority bool
	// contains filtered or unexported fields
}
```

A Server defines parameters for running an HTTP server. The zero value for Server is a valid configuration.

#### func (\*Server) [Close](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3199) added in go1.8

```
func (s *Server) Close() error
```

Close immediately closes all active net.Listeners and any connections in state [StateNew](#StateNew), [StateActive](#StateActive), or [StateIdle](#StateIdle). For a graceful shutdown, use [Server.Shutdown](#Server.Shutdown).

Close does not attempt to close (and does not even know about) any hijacked connections, such as WebSockets.

Close returns any error returned from closing the [Server](#Server)'s underlying Listener(s).

#### func (\*Server) [ListenAndServe](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3450)

```
func (s *Server) ListenAndServe() error
```

ListenAndServe listens on the TCP network address s.Addr and then calls [Serve](#Serve) to handle requests on incoming connections. Accepted connections are configured to enable TCP keep-alives.

If s.Addr is blank, ":http" is used.

ListenAndServe always returns a non-nil error. After [Server.Shutdown](#Server.Shutdown) or [Server.Close](#Server.Close), the returned error is [ErrServerClosed](#ErrServerClosed).

#### func (\*Server) [ListenAndServeTLS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3896)

```
func (s *Server) ListenAndServeTLS(certFile, keyFile string) error
```

ListenAndServeTLS listens on the TCP network address s.Addr and then calls [ServeTLS](#ServeTLS) to handle requests on incoming TLS connections. Accepted connections are configured to enable TCP keep-alives.

Filenames containing a certificate and matching private key for the server must be provided if neither the [Server](#Server)'s TLSConfig.Certificates nor TLSConfig.GetCertificate are populated. If the certificate is signed by a certificate authority, the certFile should be the concatenation of the server's certificate, any intermediates, and the CA's certificate.

If s.Addr is blank, ":https" is used.

ListenAndServeTLS always returns a non-nil error. After [Server.Shutdown](#Server.Shutdown) or [Server.Close](#Server.Close), the returned error is [ErrServerClosed](#ErrServerClosed).

#### func (\*Server) [RegisterOnShutdown](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3295) added in go1.9

```
func (s *Server) RegisterOnShutdown(f func())
```

RegisterOnShutdown registers a function to call on [Server.Shutdown](#Server.Shutdown). This can be used to gracefully shutdown connections that have undergone ALPN protocol upgrade or that have been hijacked. This function should start protocol-specific graceful shutdown, but should not wait for shutdown to complete.

#### func (\*Server) [Serve](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3506)

```
func (s *Server) Serve(l net.Listener) error
```

Serve accepts incoming connections on the Listener l, creating a new service goroutine for each. The service goroutines read requests and then call s.Handler to reply to them.

HTTP/2 support is only enabled if the Listener returns [\*tls.Conn](https://pkg.go.dev/crypto/tls#Conn) connections and they were configured with "h2" in the TLS Config.NextProtos.

Serve always returns a non-nil error and closes l. After [Server.Shutdown](#Server.Shutdown) or [Server.Close](#Server.Close), the returned error is [ErrServerClosed](#ErrServerClosed).

#### func (\*Server) [ServeTLS](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3615) added in go1.9

```
func (s *Server) ServeTLS(l net.Listener, certFile, keyFile string) error
```

ServeTLS accepts incoming connections on the Listener l, creating a new service goroutine for each. The service goroutines perform TLS setup and then read requests, calling s.Handler to reply to them.

Files containing a certificate and matching private key for the server must be provided if neither the [Server](#Server)'s TLSConfig.Certificates, TLSConfig.GetCertificate nor config.GetConfigForClient are populated. If the certificate is signed by a certificate authority, the certFile should be the concatenation of the server's certificate, any intermediates, and the CA's certificate.

ServeTLS always returns a non-nil error. After [Server.Shutdown](#Server.Shutdown) or [Server.Close](#Server.Close), the returned error is [ErrServerClosed](#ErrServerClosed).

#### func (\*Server) [SetKeepAlivesEnabled](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3771) added in go1.3

```
func (s *Server) SetKeepAlivesEnabled(v bool)
```

SetKeepAlivesEnabled controls whether HTTP keep-alives are enabled. By default, keep-alives are always enabled. Only very resource-constrained environments or servers in the process of shutting down should disable them.

#### func (\*Server) [Shutdown](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/server.go;l=3249) added in go1.8

```
func (s *Server) Shutdown(ctx context.Context) error
```

Shutdown gracefully shuts down the server without interrupting any active connections. Shutdown works by first closing all open listeners, then closing all idle connections, and then waiting indefinitely for connections to return to idle and then shut down. If the provided context expires before the shutdown is complete, Shutdown returns the context's error, otherwise it returns any error returned from closing the [Server](#Server)'s underlying Listener(s).

When Shutdown is called, [Serve](#Serve), [ServeTLS](#ServeTLS), [ListenAndServe](#ListenAndServe), and [ListenAndServeTLS](#ListenAndServeTLS) immediately return [ErrServerClosed](#ErrServerClosed). Make sure the program doesn't exit and waits instead for Shutdown to return.

Shutdown does not attempt to close nor wait for hijacked connections such as WebSockets. The caller of Shutdown should separately notify such long-lived connections of shutdown and wait for them to close, if desired. See [Server.RegisterOnShutdown](#Server.RegisterOnShutdown) for a way to register shutdown notification functions.

Once Shutdown has been called on a server, it may not be reused; future calls to methods such as Serve will return ErrServerClosed.

Example [¶](#example-Server.Shutdown "Go to Example")

```
package main

import (
	"context"
	"log"
	"net/http"
	"os"
	"os/signal"
)

func main() {
	var srv http.Server

	idleConnsClosed := make(chan struct{})
	go func() {
		sigint := make(chan os.Signal, 1)
		signal.Notify(sigint, os.Interrupt)
		<-sigint

		// We received an interrupt signal, shut down.
		if err := srv.Shutdown(context.Background()); err != nil {
			// Error from closing listeners, or context timeout:
			log.Printf("HTTP server Shutdown: %v", err)
		}
		close(idleConnsClosed)
	}()

	if err := srv.ListenAndServe(); err != http.ErrServerClosed {
		// Error starting or closing listener:
		log.Fatalf("HTTP server ListenAndServe: %v", err)
	}

	<-idleConnsClosed
}
```




#### type [Transport](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=98)

```
type Transport struct {

	// Proxy specifies a function to return a proxy for a given
	// Request. If the function returns a non-nil error, the
	// request is aborted with the provided error.
	//
	// The proxy type is determined by the URL scheme. "http",
	// "https", "socks5", and "socks5h" are supported. If the scheme is empty,
	// "http" is assumed.
	// "socks5" is treated the same as "socks5h".
	//
	// If the proxy URL contains a userinfo subcomponent,
	// the proxy request will pass the username and password
	// in a Proxy-Authorization header.
	//
	// If Proxy is nil or returns a nil *URL, no proxy is used.
	Proxy func(*Request) (*url.URL, error)

	// OnProxyConnectResponse is called when the Transport gets an HTTP response from
	// a proxy for a CONNECT request. It's called before the check for a 200 OK response.
	// If it returns an error, the request fails with that error.
	OnProxyConnectResponse func(ctx context.Context, proxyURL *url.URL, connectReq *Request, connectRes *Response) error

	// DialContext specifies the dial function for creating unencrypted TCP connections.
	// If DialContext is nil (and the deprecated Dial below is also nil),
	// then the transport dials using package net.
	//
	// DialContext runs concurrently with calls to RoundTrip.
	// A RoundTrip call that initiates a dial may end up using
	// a connection dialed previously when the earlier connection
	// becomes idle before the later DialContext completes.
	DialContext func(ctx context.Context, network, addr string) (net.Conn, error)

	// Dial specifies the dial function for creating unencrypted TCP connections.
	//
	// Dial runs concurrently with calls to RoundTrip.
	// A RoundTrip call that initiates a dial may end up using
	// a connection dialed previously when the earlier connection
	// becomes idle before the later Dial completes.
	//
	// Deprecated: Use DialContext instead, which allows the transport
	// to cancel dials as soon as they are no longer needed.
	// If both are set, DialContext takes priority.
	Dial func(network, addr string) (net.Conn, error)

	// DialTLSContext specifies an optional dial function for creating
	// TLS connections for non-proxied HTTPS requests.
	//
	// If DialTLSContext is nil (and the deprecated DialTLS below is also nil),
	// DialContext and TLSClientConfig are used.
	//
	// If DialTLSContext is set, the Dial and DialContext hooks are not used for HTTPS
	// requests and the TLSClientConfig and TLSHandshakeTimeout
	// are ignored. The returned net.Conn is assumed to already be
	// past the TLS handshake.
	//
	// To support ALPN protocol negotiation, the returned net.Conn should be
	// a *tls.Conn or implement the same ConnectionState method as *tls.Conn.
	DialTLSContext func(ctx context.Context, network, addr string) (net.Conn, error)

	// DialTLS specifies an optional dial function for creating
	// TLS connections for non-proxied HTTPS requests.
	//
	// Deprecated: Use DialTLSContext instead, which allows the transport
	// to cancel dials as soon as they are no longer needed.
	// If both are set, DialTLSContext takes priority.
	DialTLS func(network, addr string) (net.Conn, error)

	// TLSClientConfig specifies the TLS configuration to use with
	// tls.Client.
	// If nil, the default configuration is used.
	// If non-nil, HTTP/2 support may not be enabled by default.
	TLSClientConfig *tls.Config

	// TLSHandshakeTimeout specifies the maximum amount of time to
	// wait for a TLS handshake. Zero means no timeout.
	TLSHandshakeTimeout time.Duration

	// DisableKeepAlives, if true, disables HTTP keep-alives and
	// will only use the connection to the server for a single
	// HTTP request.
	//
	// This is unrelated to the similarly named TCP keep-alives.
	DisableKeepAlives bool

	// DisableCompression, if true, prevents the Transport from
	// requesting compression with an "Accept-Encoding: gzip"
	// request header when the Request contains no existing
	// Accept-Encoding value. If the Transport requests gzip on
	// its own and gets a gzipped response, it's transparently
	// decoded in the Response.Body. However, if the user
	// explicitly requested gzip it is not automatically
	// uncompressed.
	DisableCompression bool

	// MaxIdleConns controls the maximum number of idle (keep-alive)
	// connections across all hosts. Zero means no limit.
	MaxIdleConns int

	// MaxIdleConnsPerHost, if non-zero, controls the maximum idle
	// (keep-alive) connections to keep per-host. If zero,
	// DefaultMaxIdleConnsPerHost is used.
	MaxIdleConnsPerHost int

	// MaxConnsPerHost optionally limits the total number of
	// connections per host, including connections in the dialing,
	// active, and idle states. On limit violation, dials will block.
	//
	// Zero means no limit.
	MaxConnsPerHost int

	// IdleConnTimeout is the maximum amount of time an idle
	// (keep-alive) connection will remain idle before closing
	// itself.
	// Zero means no limit.
	IdleConnTimeout time.Duration

	// ResponseHeaderTimeout, if non-zero, specifies the amount of
	// time to wait for a server's response headers after fully
	// writing the request (including its body, if any). This
	// time does not include the time to read the response body.
	ResponseHeaderTimeout time.Duration

	// ExpectContinueTimeout, if non-zero, specifies the amount of
	// time to wait for a server's first response headers after fully
	// writing the request headers if the request has an
	// "Expect: 100-continue" header. Zero means no timeout and
	// causes the body to be sent immediately, without
	// waiting for the server to approve.
	// This time does not include the time to send the request header.
	ExpectContinueTimeout time.Duration

	// TLSNextProto specifies how the Transport switches to an
	// alternate protocol (such as HTTP/2) after a TLS ALPN
	// protocol negotiation. If Transport dials a TLS connection
	// with a non-empty protocol name and TLSNextProto contains a
	// map entry for that key (such as "h2"), then the func is
	// called with the request's authority (such as "example.com"
	// or "example.com:1234") and the TLS connection. The function
	// must return a RoundTripper that then handles the request.
	// If TLSNextProto is not nil, HTTP/2 support is not enabled
	// automatically.
	//
	// Historically, TLSNextProto was used to disable HTTP/2 support.
	// The Transport.Protocols field now provides a simpler way to do this.
	TLSNextProto map[string]func(authority string, c *tls.Conn) RoundTripper

	// ProxyConnectHeader optionally specifies headers to send to
	// proxies during CONNECT requests.
	// To set the header dynamically, see GetProxyConnectHeader.
	ProxyConnectHeader Header

	// GetProxyConnectHeader optionally specifies a func to return
	// headers to send to proxyURL during a CONNECT request to the
	// ip:port target.
	// If it returns an error, the Transport's RoundTrip fails with
	// that error. It can return (nil, nil) to not add headers.
	// If GetProxyConnectHeader is non-nil, ProxyConnectHeader is
	// ignored.
	GetProxyConnectHeader func(ctx context.Context, proxyURL *url.URL, target string) (Header, error)

	// MaxResponseHeaderBytes specifies a limit on how many
	// response bytes are allowed in the server's response
	// header.
	//
	// Zero means to use a default limit.
	MaxResponseHeaderBytes int64

	// WriteBufferSize specifies the size of the write buffer used
	// when writing to the transport.
	// If zero, a default (currently 4KB) is used.
	WriteBufferSize int

	// ReadBufferSize specifies the size of the read buffer used
	// when reading from the transport.
	// If zero, a default (currently 4KB) is used.
	ReadBufferSize int

	// ForceAttemptHTTP2 controls whether HTTP/2 is enabled when a non-zero
	// Dial, DialTLS, or DialContext func or TLSClientConfig is provided.
	// By default, use of any those fields conservatively disables HTTP/2.
	// To use a custom dialer or TLS config and still attempt HTTP/2
	// upgrades, set this to true.
	ForceAttemptHTTP2 bool

	// HTTP2 configures HTTP/2 connections.
	HTTP2 *HTTP2Config

	// Protocols is the set of protocols supported by the transport.
	//
	// If Protocols includes UnencryptedHTTP2 and does not include HTTP1,
	// the transport will use unencrypted HTTP/2 for requests for http:// URLs.
	//
	// If Protocols is nil, the default is usually HTTP/1 only.
	// If ForceAttemptHTTP2 is true, or if TLSNextProto contains an "h2" entry,
	// the default is HTTP/1 and HTTP/2.
	Protocols *Protocols
	// contains filtered or unexported fields
}
```

Transport is an implementation of [RoundTripper](#RoundTripper) that supports HTTP, HTTPS, and HTTP proxies (for either HTTP or HTTPS with CONNECT).

By default, Transport caches connections for future re-use. This may leave many open connections when accessing many hosts. This behavior can be managed using [Transport.CloseIdleConnections](#Transport.CloseIdleConnections) method and the [Transport.MaxIdleConnsPerHost](#Transport.MaxIdleConnsPerHost) and [Transport.DisableKeepAlives](#Transport.DisableKeepAlives) fields.

Transports should be reused instead of created as needed. Transports are safe for concurrent use by multiple goroutines.

A Transport is a low-level primitive for making HTTP and HTTPS requests. For high-level functionality, such as cookies and redirects, see [Client](#Client).

Transport uses HTTP/1.1 for HTTP URLs and either HTTP/1.1 or HTTP/2 for HTTPS URLs, depending on whether the server supports HTTP/2, and how the Transport is configured. The [DefaultTransport](#DefaultTransport) supports HTTP/2. To explicitly enable HTTP/2 on a transport, set [Transport.Protocols](#Transport.Protocols).

Responses with status codes in the 1xx range are either handled automatically (100 expect-continue) or ignored. The one exception is HTTP status code 101 (Switching Protocols), which is considered a terminal status and returned by [Transport.RoundTrip](#Transport.RoundTrip). To see the ignored 1xx responses, use the httptrace trace package's ClientTrace.Got1xxResponse.

Transport only retries a request upon encountering a network error if the connection has already been used successfully and if the request is idempotent and either has no body or has its [Request.GetBody](#Request.GetBody) defined. HTTP requests are considered idempotent if they have HTTP methods GET, HEAD, OPTIONS, or TRACE; or if their [Header](#Header) map contains an "Idempotency-Key" or "X-Idempotency-Key" entry. If the idempotency key value is a zero-length slice, the request is treated as idempotent but the header is not sent on the wire.

####  func (\*Transport) [CancelRequest](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=1018) deprecated added in go1.1

```
func (t *Transport) CancelRequest(req *Request)
```

CancelRequest cancels an in-flight request by closing its connection. CancelRequest should only be called after [Transport.RoundTrip](#Transport.RoundTrip) has returned.

Deprecated: Use [Request.WithContext](#Request.WithContext) to create a request with a cancelable context instead. CancelRequest cannot cancel HTTP/2 requests. This may become a no-op in a future release of Go.


#### func (\*Transport) [Clone](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=344) added in go1.13

```
func (t *Transport) Clone() *Transport
```

Clone returns a deep copy of t's exported fields.

#### func (\*Transport) [CloseIdleConnections](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=949)

```
func (t *Transport) CloseIdleConnections()
```

CloseIdleConnections closes any connections which were previously connected from previous requests but are now sitting idle in a "keep-alive" state. It does not interrupt any connections currently in use.

#### func (\*Transport) [NewClientConn](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/clientconn.go;l=113) added in go1.26.0

```
func (t *Transport) NewClientConn(ctx context.Context, scheme, address string) (*ClientConn, error)
```

NewClientConn creates a new client connection to the given address.

If scheme is "http", the connection is unencrypted. If scheme is "https", the connection uses TLS.

The protocol used for the new connection is determined by the scheme, Transport.Protocols configuration field, and protocols supported by the server. See Transport.Protocols for more details.

If Transport.Proxy is set and indicates that a request sent to the given address should use a proxy, the new connection uses that proxy.

NewClientConn always creates a new connection, even if the Transport has an existing cached connection to the given host.

The new connection is not added to the Transport's connection cache, and will not be used by [Transport.RoundTrip](#Transport.RoundTrip). It does not count against the MaxIdleConns and MaxConnsPerHost limits.

The caller is responsible for closing the new connection.

#### func (\*Transport) [RegisterProtocol](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/transport.go;l=904)

```
func (t *Transport) RegisterProtocol(scheme string, rt RoundTripper)
```

RegisterProtocol registers a new protocol with scheme. The [Transport](#Transport) will pass requests using the given scheme to rt. It is rt's responsibility to simulate HTTP request semantics.

RegisterProtocol can be used by other packages to provide implementations of protocol schemes like "ftp" or "file".

If rt.RoundTrip returns [ErrSkipAltProtocol](#ErrSkipAltProtocol), the Transport will handle the [Transport.RoundTrip](#Transport.RoundTrip) itself for that one request, as if the protocol were not registered.

#### func (\*Transport) [RoundTrip](https://cs.opensource.google/go/go/+/go1.27.0:src/net/http/roundtrip.go;l=29)

```
func (t *Transport) RoundTrip(req *Request) (*Response, error)
```

RoundTrip implements the [RoundTripper](#RoundTripper) interface.

For higher-level HTTP client support (such as handling of cookies and redirects), see [Get](#Get), [Post](#Post), and the [Client](#Client) type.

Like the RoundTripper interface, the error types returned by RoundTrip are unspecified.
