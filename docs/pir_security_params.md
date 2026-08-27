# Why We Doubled PIR's Security Parameter for Zcash Voting

> Source: https://x.com/compose/articles/edit/2093108171413852160/preview

In the past few weeks, several discussions have highlighted the risks of lattice cryptography. [Published papers](https://eprint.iacr.org/2026/279) claimed reductions of several bits of security in schemes that were previously deemed standard.

Some of these recent attacks were later [formally refuted](https://eprint.iacr.org/2026/1693). Others [applied only under specific parameters](https://eprint.iacr.org/2026/279). None demonstrated a practical break of private information retrieval (PIR).

That wasn't our bar.

With the [Zcash token holder vote](https://forum.zcashcommunity.com/t/the-coinholder-voting-chain/56925) going live, lattice-based PIR is moving into production. We proactively analyzed the latest research and updated our parameters to retain a major security buffer, including against quantum attacks.

This post explains what concerned us, how we evaluated it, and why we doubled the main security parameter before launch.

First, let's discuss the application.

The Zcash voting system allows token holders to express their opinions on the future of the protocol. The rule is 1 ZEC = 1 vote. Users zk-prove their balance at a snapshot height, attesting that the corresponding note has not been spent.

To assert the latter, a user needs a Merkle non-existence proof for their nullifier. The proof comes from an off-chain server and checks against a root published on the voting side-chain.

The problem: if a user directly queried the server for their nullifier, the server could later link it to the nullifier published on Zcash mainnet as part of a spend.

[Image](https://pbs.twimg.com/media/HQw-APSWMAAK2m9.jpg)Nullifier PIR Protocol

![Image](https://pbs.twimg.com/media/HQw-APSWMAAK2m9?format=jpg&name=orig)

Nullifier PIR Protocol

We solve this with [YPIR+SP](https://github.com/valargroup/ypir), a lattice-based PIR scheme.

Its rough intuition: homomorphically matrix-vector multiply an encrypted client query against a public database. The response remains encrypted. Use another [cryptography trick](https://eprint.iacr.org/2020/015) to compress it. The client decrypts their Merkle proof, while the requested row remains computationally hidden from the server.

I previously covered the underlying [LWE and RLWE cryptography](https://x.com/akhtariev/status/2031505309387129169) and the broader [YPIR security model](https://x.com/akhtariev/status/2030768109196316712). Here, we will focus only on the security parameters.

# Measuring Security

YPIR uses Regev encryption, whose security is based on the [Learning With Errors (LWE)](https://en.wikipedia.org/wiki/Learning_with_errors) problem.

The general practice is to assess lattice security by [modeling every relevant known attack](https://github.com/malb/lattice-estimator) against our exact configuration, taking the cheapest result. We then add margin because estimators are imperfect and future attacks may improve.

Lattice dimension is a key variable, but not the only one. The modulus, noise, secret distribution, available samples, and algebraic structure all influence the security argument.

The YPIR paper claims [128-bit security](https://github.com/menonsamir/ypir/issues/1#issuecomment-1967365470) for its default parameters. Roughly, this means that no known modeled attack is expected to cost less than 2^128 work. It is an estimate, not a proof or a permanent guarantee.

Interestingly, the result also depends on how you count the "work." There are several cost models. Two examples:

- Core-SVP prices calls to the hardest underlying lattice operation. This makes schemes easy to compare, but omits many real attack costs and is deliberately conservative.
- MATZOV is a more concrete model. It accounts for more of the work performed by an attack, including variables such as lattice dimension and available LWE samples.

Both are estimators rather than deterministic measures of security. Their outputs should not be read as equivalent wall-clock costs.

When [rerunning the default YPIR parameters](https://github.com/valargroup/vote-nullifier-pir/blob/d28745c82924e3c5ffb3839334eb1076cb9635cd/docs/security/nullifier-pir-analysis.py), we saw estimates ranging from 94-bit quantum Core-SVP to 131-bit MATZOV. The lower figure is a conservative abstraction rather than a demonstrated 2^94 attack. Still, it fell below the margin we wanted for a new production deployment.

This did not meet our confidence bar, so we kept digging.

# More Structure: RLWE

YPIR does not use only unstructured LWE.

LWE is efficient for the server's first matrix-vector multiplication. However, sending the result naively would require the client to download a large database-dependent hint. [Prior PIR schemes](https://eprint.iacr.org/2022/949) required a 121 MB hint for a 1 GB database. Completely impractical for production.

YPIR solves this through another trick: it embeds the LWE results into the [negacyclic structure of a polynomial ring](https://x.com/akhtariev/status/2031505309387129169) and compresses them into an RLWE ciphertext. This homomorphic trick is called the [CDKS transformation](https://eprint.iacr.org/2020/015).

That structure is useful, but structure can also give an attacker more to work with.

Security estimates for RLWE and Module-LWE are commonly obtained by translating the parameters into an "equivalent" unstructured LWE instance. [A recent paper](https://eprint.iacr.org/2026/279) challenged the assumption that the additional ring structure is free. It showed that cyclotomic rotations can strengthen hybrid attacks, reporting a consistent 2–3 bit gap for ML-KEM and larger gaps for some sparse-secret RLWE parameters.

These results do not directly break YPIR, and the parameters are not identical. However, they demonstrate why an unstructured LWE estimate should not be treated as the final word for a ring-based construction.

The default estimates already ranged from 94 to 131 bits, depending on the cost model. Recent research introduced more uncertainty around the structured part used for compression. This wasn't appropriate by our standards.

# Updated Security

For launch, we chose to double the lattice dimension from the default 2048 to 4096.

[Image](https://pbs.twimg.com/media/HQxAg6gXQAACPek.png)Security update

![Image](https://pbs.twimg.com/media/HQxAg6gXQAACPek?format=png&name=orig)

Security update

Rerunning the same conservative baseline produced:

- Quantum Core-SVP: 238.5 bits
- Classical Core-SVP: 262.8 bits

These numbers should not be interpreted as a proof that breaking the scheme requires exactly 2^239 or 2^263 operations. They are modeled estimates against known attacks.

The important outcome is the margin. Even if future work improves attacks, exploits more of the ring structure, or changes how costs are modeled, the updated parameters leave substantially more room than the defaults.

## Performance Impact and Production Deployment

Stronger parameters have a cost. By increasing lattice dimension from 2048 to 4096, the query upload increased from 544 KB to 1.53 MB. On average, one vote needs 5 of these, so 8.27 MB total, roughly 2.5x.

[Image](https://pbs.twimg.com/media/HQw-dTpWMAApAUy.png)Performance Change

![Image](https://pbs.twimg.com/media/HQw-dTpWMAApAUy?format=png&name=orig)

Performance Change

The new Ironwood pool gave us room to make that trade. The previous PIR design was sized for roughly 67 million Orchard nullifiers. At voting system launch, Ironwood had only about 31,000. Our redesigned tree reduced the private database to 48 MiB and replaced two sequential PIR queries with one. We chose to spend part of that performance dividend on security margin.

The resulting system remained practical. In a 60-second load test at eight concurrent requests, the 4096-degree configuration completed 689 proofs with zero errors: 11.48 proofs per second, 685 ms median latency, and 975 ms p99 latency.

We optimized for a security margin that would prioritize making the system private and secure for the end user. In future deployments, efforts will be made to improve PIR UX by making it faster, with no security compromise. You can read more about our performance improvements across the stack [here](https://zakura.com/engineering/).

# Closing Thoughts

No parameter choice makes lattice cryptography permanently secure. Security estimates evolve as attacks, implementations, and cost models improve.

The correct response is not to assume that a published parameter set remains sufficient forever. But proactively and regularly update the security model behind your protocol

That is the standard our team at Valar Group applies to all software we develop.

To test out, download [Vizor wallet](https://vizor.cash/) and participate in the ongoing [NU7 token-holder vote](https://forum.zcashcommunity.com/t/nu7-coinholder-vote/56912).

## Credits

Shout out to [Stardust Staking](https://starduststaking.com/) for operating production PIR vote servers along with Valar Group.

The voting system has been developed by [@ValarGroup](https://x.com/@ValarGroup) under the leadership of [@zkDragon](https://x.com/@zkDragon).
