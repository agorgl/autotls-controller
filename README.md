# autotls-controller

A kubernetes controller for automatic tls configuration written in Rust.

## Using

To automatically setup tls for an ingress, annotate the ingress with `autotls/issuer` annotation,
passing the cluster issuer that will issue the certificate or `auto` to use the default issuer:
```
kubectl annotate ingress sample autotls/issuer=auto
```

To automatically append a domain in some ingress's rules, annotate the ingress with `autotls/domain` annotation,
passing the domain to be appended in the hosts that do not contain one:
```
kubectl annotate ingress sample autotls/domain=example.com
```
