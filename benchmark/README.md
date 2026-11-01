# Benchmark

Per call, xslash adds 0.763 ms over running the binary directly for simple
archives like go (1.894 → 2.657 ms) and 0.821 ms for composed artifacts like
rust (4.555 → 5.376 ms).

That ties DotSlash, which does the same work: only 0.009 ms difference per call.

At under a millisecond per call, the overhead is almost imperceptible to a
human.

```
# go

## raw (10000x)
1.894 ms/call

## path (10000x)
1.903 ms/call

## single (10000x)
2.654 ms/call

## final (10000x)
2.657 ms/call

## discover (10000x)
2.698 ms/call

## inject (10000x)
2.717 ms/call

## dotslash (10000x)
2.666 ms/call

# rust

## raw (10000x)
4.555 ms/call

## final (10000x)
5.376 ms/call
```
