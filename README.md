# GeoDnsProxy

Tiny DNS proxy made for a university lab about CDNs.  
GeoDnsProxy proxies UDP DNS traffic based on origin IP address to defined name servers for that zone.

## Configuration
An example config is provided and can be edited to your needs.

## Potential Problems
* A malicious party can try to guess the transaction ID in the DNS header (16 bit) of an ongoing request and responses may go to the wrong party or get dropped.
* Slow connections heavily impact the performance as only one incoming and outgoing socket are used