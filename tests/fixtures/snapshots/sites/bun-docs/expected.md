Learn the basic request flow for this runtime API.

The runtime accepts a request, validates its options, and returns a response that the caller can inspect.

```
const result = runtime.request({ method: "GET" });
```

## Configure the request

Set the endpoint and timeout before sending the request. The client keeps these values together so each operation is predictable.

```
const client = runtime.createClient({ timeout: 5000 });
```

## Handle errors

Check the returned status and handle failures close to the request. This keeps recovery behavior visible to the caller.

```
if (!result.ok) throw new Error(result.status);
```

### Configure retries

Retry only transient failures and keep the retry count bounded so an unavailable service does not create an endless loop.

```
const retry = runtime.retry({ attempts: 3 });
```

### Enable logging

Enable request logging when you need to inspect a failed operation. The log records the method, status, and elapsed time.

```
const log = runtime.logger({ level: "info" });
```

### Close the client

Close the client after the final operation to release its resources and make shutdown behavior explicit.

```
await client.close();
```

---

## Advanced options

Advanced options extend the same request flow without changing the response contract or error handling rules.

```
const advanced = runtime.request({ cache: "no-store" });
```
