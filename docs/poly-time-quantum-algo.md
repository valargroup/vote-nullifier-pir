---
title: "A Polynomial-Time Quantum Algorithm for the Dihedral Coset Problem"
subtitle: "Preliminary Draft"
author: "Daniel R. Simon"
date: "2026-07-31"
eprint: "2026/1591"
source_pdf: "poly-time-quantum-algo.pdf"
source_url: "https://eprint.iacr.org/2026/1591"
license: "CC BY"
format: "AI-readable Markdown transcription"
---

# A Polynomial-Time Quantum Algorithm for the Dihedral Coset Problem

**Status:** Preliminary draft  
**Author:** Daniel R. Simon  
**Affiliation:** Amazon Web Services, Cryptography Group  
**Contact:** `dcp-paper@amazon.com`  
**Date:** July 31, 2026

> Conversion note: This is a structured text transcription of the 16-page PDF. Section hierarchy, equations, definitions, lemmas, proof sketches, references, and page boundaries are retained as text. PDF extraction can flatten typography in dense quantum-state expressions; consult the source PDF whenever exact superscript, subscript, summation, or ket notation is material.

## AI navigation note

_This note is editorial metadata, not part of the authored paper._

- **Claimed result:** A polynomial-time quantum algorithm for the Dihedral Coset Problem (DCP) that avoids Regev’s subset-sum oracle.
- **Core technique:** Partition Fourier-transformed samples into groups, use Hadamard measurements to form sets `A` and `B`, construct a replacement high-order qubit `h*`, and transfer the phase encoding the hidden bit from `h` to `h*`.
- **Claimed fault tolerance:** Faulty-sample probability up to `1/O(log n)`.
- **Claimed lattice consequence:** Via cited reductions, polynomial-time algorithms for `O(sqrt(n) polylog(n))`-approximate SVP and corresponding LWE instances.
- **Proof structure:** One main theorem, four lemmas, a corollary on amplitude bounds, and a final lattice-problem corollary.
- **Status warning:** This is explicitly a preliminary draft. The transcription records the paper’s claims and is not independent validation of the algorithm or reductions.

## Abstract

We present a polynomial-time quantum algorithm for the Dihedral Coset Problem (DCP). The algorithm is based on Regev’s polynomial-time reduction of the Dihedral Subgroup Problem (DSP) to the modular subset sum problem [10], but uses a different technique to erase sample bits without use of a subset sum oracle. The algorithm can thus combine with Regev’s [10] reduction of lattice problems to DCP, improved by Brakerski, Kirshanova, Stehlé and Wen [3], to yield polynomial-time quantum algorithms for various lattice problems, such as finding a polynomial-factor approximation to the shortest vector in an n-dimensional lattice (SVP), and the “learning with errors” problem (LWE). The algorithm can tolerate a faulty sample rate as high as $1/O(\log n)$, allowing the algorithm-reduction combination to efficiently solve, for example, SVP with a $\sqrt{n}\operatorname{polylog}(n)$ approximation factor, or LWE instances with $\alpha = \sqrt{n}\operatorname{polylog}(n)$.

## 1 Introduction

### 1.1 Lattice-Based Cryptography

The discovery of quantum algorithms capable of efficiently attacking the most popular classical public-key cryptosystems [12] has created interest in public-key cryptosystems that are resistant to such quantum attacks. One promising approach is “lattice cryptography” [8], which is based on the believed computational difficulty of various problems over lattices, such as the polynomial-factor approximate Shortest Vector Problem (p(n)-SVP). p(n)-SVP is the problem of finding a non-zero vector in an n-dimensional

<!-- PDF page 1 -->

lattice whose length is within a polynomial p(n) factor of the length of the shortest non-zero vector in the lattice. The security of several lattice-based cryptosystems, such as the Ajtai-Dwork cryptosystem [1] and “learning with errors” (LWE) [11], depends on the hardness of p(n)-SVP. The best previously available algorithms—classical or quantum—for solving p(n)-SVP, such as BKZ ([4]), require fully exponential time.

### 1.2 The Dihedral Subgroup Problem
The Dihedral Subgroup Problem (DSP) [5] can be stated as follows: assume
that a function f is constant on a subgroup H of a dihedral group G, as
well as on each of its cosets; the problem is to identify H. Ettinger and
Høyer [5] showed that it suffices to solve the problem in the case where the
subgroup is of order 2. Applying the standard “sampling” method results
in the following very simple formulation of the problem, due to Regev [10]:
given a source of random samples of the superposition

$$
\frac{1}{\sqrt{2}}\lvert 0,x\rangle
+ \frac{1}{\sqrt{2}}\lvert 1,x+d \pmod N\rangle
$$

where 2N ≈ 2 n+1 is the size of the dihedral group, d, x ∈ { 0...N − 1} , d is fixed and x is chosen arbitrarily for each sample, find d. Boneh and Lipton [2] showed that the general hidden subgroup problem for Abelian groups is in quantum polynomial time, but dihedral groups are (slightly) non-Abelian. The best previously known quantum algorithm for solving DSP is due to Kuperberg [6], and has running time 2 O(√log N) ≈ 2 O(√n). Regev [10] gives a polynomial-time quantum algorithm for DSP, but it relies on a subset sum oracle to “erase” input bits. The algorithm presented here emulates Regev’s algorithm, but handles the input bits differently.

### 1.3 The Dihedral Coset Problem

The Dihedral Coset Problem (DCP) [10] is identical to Regev’s formulation of the DSP, but with a faulty sample probability of 1/a(n) in the DSP samples—that is, with probability 1/a(n) that the sample will consist of a random bit and random value, rather than the correct superposition. Regev [10] showed a polynomial-time reduction from solving a(n)-SVP to solving DCP on a group of size 2N, where N ≈ 2 n 2 . This quadratic dimension increase was later removed in an improved reduction from LWE presented by Brakerski, Kirshanova, Stehl´e and Wen [3]. These reductions both yield an approximation factor a(n)√n polylog(n), but at the cost of a 1/a(n) faulty

<!-- PDF page 2 -->

sample probability. Kuperberg’s algorithm requires error-free input, and hence produces an approximation factor at best 2 O(√n) when combined with Regev’s reduction—no better than what BKZ offers classically. Since the quantum algorithm presented here tolerates a faulty sample rate as high as 1/O(log n), it can produce (via [3]), for example, a polynomial-time quantum algorithm for SVP with an approximation factor √npolylog(n), or for LWE instances with α =√n polylog(n) [9] [7].

## 2 The Algorithm

### 2.1 Algorithm Overview
Regev’s polynomial-time DSP algorithm [10] that uses a subset sum oracle
can be summarized as follows: first, generate slightly more than n DSP
samples

P bi∈{ 0,1}| bi, x+bid⟩ (all arithmetic operations being mod N, where N ≈ 2 n), then Fourier-transform the x + bid portion of each and measure it, producing a superposition of 0 and 1 and a measured value yi, such that the phases of 0 and 1 differ by a factor ω yid (where ω is an Nth root of unity). Next, compute the subset sum P

biyi in superposition, and measure all but the most significant bit h of it, calling the measured bits z′. The phases of all of the states that survive this measurement will be

ωP

biyid = ω (z′+hN/2)d, where ω z′d is constant. We can then use the subset sum oracle to compute (in superposition) the solutions to the two subset sum problems

P

biyi = z′ + hN/2 given z′ for h = 0 and h = 1. With non-negligible probability, there will be exactly one solution for each value of h, which can be used to “erase” the bi values in each case, leaving only h distinguishing the two solution states. Their phases will differ by ω Nd/2, which will be 1 if the last bit dn of d is 0 and − 1 if dn = 1. Hence Hadamard transforming h and measuring it will produce dn with probability 1. The remaining bits of d can be obtained by repeating this algorithm recursively, using knowledge of dn to erase the last bit from every sample and obtaining dn− 1 as above, and so on. We will follow this outline, but use a different method to erase the bi values. First, as before, we will generate DSP samples

P bi∈{ 0,1}| bi, x + bid⟩ (some of them faulty—i.e., a random (n + 1)-bit value, not in superposition), Fourier-transform the x + bid portions yi, and compute the subset sum z in superposition, measuring the least significant n − 1 bits z′ of the sum, while leaving the most significant bit h unmeasured. However, we will do this with kn c+1 samples (for constants k and c), forming a superposition of states ϕ = b1...bknc+1 . We then process the bits in groups of c log n, as

<!-- PDF page 3 -->

follows: first, we compute in superposition the log n most significant bits s of the sum rj =

P

biyi for group gj , then we Hadamard-transform the bi values in gj and measure them along with the corresponding yi values. If all the bi measurements result in 0, then we include this group in set A—otherwise, we will measure all of the bits of s for the group, and then include it in set B. Our goal will be to run Regev’s algorithm on A—which has no extraneous phases introduced by the erasure of its bits bi—instead of on the entire set of samples, using the s values for A to construct a new h∗ that replaces the original h, and transferring the phase that encodes dn from h to h∗. The lower-order n − 1 bits of the replacement subset sum z∗ =

P

Abiyi over just A won’t ever be explicitly calculated—it’ll range over all possible values, in fact—but it’ll be used implicitly to argue that the samples in B have an identical effect regardless of the value of h∗, and hence can be ignored. We can successfully collect an expected a = n/ log n groups in set A, assuming k > c and thus enough samples that an overwhelmingly large fraction of the possible measured strings contain at least that many groups qualifying for set A. We then compute the sum s∗ of the s values, measure all but the last s value sa, and erase sa using s∗ and the other measured s values. Next, we Hadamard-transform and measure all the bits of s∗ except the most significant bit h∗. Assuming we get the desired all-zeroes result (with expected probability 2/n over choices of the yi’s), we’re left with the single bits h and h∗, where h∗ divides all the states based on whether z∗ has highest-order bit 0 or 1. (Because we’ve used the most significant log n bits of the sums for each group, and there are only n/ log n s values summed to obtain s∗, the computed value of h∗ will be the correct highest-order bit of z∗, as long as the most significant log log n bits of s∗ contained at least one 0—a condition we can check for.) Finally, we compute and measure h′ = h ⊕ h∗. Recall that h starts off encoding dn in the Hadamard basis—that is, that the h = 0 and h = 1 states have the same or opposite phases depending on whether dn = 0 or dn = 1. If for each value of h′ the effect of Hadamard-transforming the bi’s in set B on the phases of h∗ = 0 and h∗ = 1 is the same, then it’s easy to verify that the computation of h′ effectively copies h’s encoding of dn to h∗. (Whether the bits of h and h∗ are exactly the same or exactly opposite, the relationship between the final phases of their two states will be the same for both qubits.) We can thus use h′ and h∗ to erase h, leaving a single unmeasured bit h∗, and measure h∗ in the Hadamard basis, in place of h, to obtain dn. All that remains is to show that the effect of set B on the amplitudes and phases of the states h∗ = 0 and h∗ = 1 is approximately equal for both. Now, these h∗ = 0 and h∗ = 1 states are each composed of the sum of all

<!-- PDF page 4 -->

the original states ϕ (each with phase 1 or − 1) consistent with the measured values and that value of h∗. We can consider each ϕ to be of the form (ϕA, ϕB), where ϕA consists of the portion of the original state associated with groups of samples in A, and similarly for ϕB. We can then partition these state-portions ϕA and ϕB into sets based on their corresponding value of z∗. We make the following observations: 1. For any particular value of z¯∗, the least significant n − 1 bits of z∗, the same set of ϕB state-portions are consistent with z¯∗ and h∗ irrespective of the value of h∗. For example, if h′ was measured to be 0 and ϕB is consistent with the value of z¯∗ and h∗ = 0, then it will also be consistent with z¯∗ and h∗ = 1, because adding 2 n− 1 to both z∗ and z′ won’t change their difference (mod N). The phases of the sums of the states ϕ consistent with z¯∗ for both values of h∗ will thus be the same, once h′ is computed and measured, and the contributions of the ϕB state portions to each amplitude will also be identical for both values of h∗. 2. Let z∗h be the most significant log n bits of z∗, and z∗l be the remaining bits. Then for a sufficiently large constant c and any z∗h, the state portions ϕA are extremely close to uniformly distributed across values of z∗l. This is a result of the pairwise independence of the subset sums z∗l [10]. (There are approximately 2 (cn(1− 1/O(log n))− n− c log n ϕA values distributed over the 2 n− log n possible z∗l values—assuming a 1/O(log n) faulty sample rate—so the expected 2 (cn(1− 1/O(log n))− 2n− O(log n) states per value will vary by a standard deviation which is the square root of that number, and which can be made exponentially small compared to the total—even when totaled over all the possible values of z∗l —by choosing a large enough c.) 3. Moreover, for any particular pair of values of z∗h that differ only in h∗, the number of ϕA state-portions consistent with each is approximately equal—differing by an expected fraction about 1/n (c− 1)/2 of the total. This is again a result of the pairwise independence of subset sums, this time in the group a that produces sa. (We consider here only the randomness in z∗h associated with the generation of sa—the additional randomness stemming from “overflow” bits generated by summing the lower n − log n bits of the subset sum only increases the entropy, and hence the uniformity, of z∗h. We also assume that ga’s c log n samples are all non-faulty, which is true with probability approximately e− c/c′, where the faulty sample rate is 1/c′log n.)

<!-- PDF page 5 -->

It follows that for any pair of values of z∗h differing only in h∗, the expected difference in the sizes of their amplitudes will be on average about 1/n (c− 1)/2—since the amplitudes for pairs of corresponding z∗l values will all be almost exactly the same, by observations 1 and 2, and with the same phase, by observation 1—and hence the total expected difference in the magnitudes of the amplitudes of h∗ = 0 and h∗ = 1 will average about 1/n (c− 3)/2. (We’re assuming here that the amplitudes associated with each z∗ value are “well-behaved” enough—that is, that they aren’t so huge and mutually canceling—that the tiny differences among amplitudes associated with the ϕA state-portions for different z∗ values aren’t unduly magnified. Fortunately, the pairwise independence of the phases of states over possible measured values of the Hadamard-transformed bits bi ensures that these per z∗ amplitudes are roughly normally distributed, and hence within reasonable bounds with high probability.) For large enough c, then, this total difference in amplitude will be small enough that we can (by repeating the algorithm multiple times) recover dn from h∗ as we did from h in the original algorithm, and recurse as before to recover all of d.

### 2.2 Algorithm Details
Theorem. There exist a polynomial-time (in n) quantum algorithm and
constant c′ such that, given DSP samples of the form

P bi∈{ 0,1}| bi, x + bid (mod N)⟩ (where N is exponential in n and x is a random value (mod N)) with probability at least 1 − 1/c′log n, and a random (n + 1)-bit value otherwise, computes the last bit dn of d. Proof. For simplicity, let N = 2 n, and assume all arithmetic operations and relations involving states, amplitudes and phases are (mod N). We will describe each step of the algorithm in sequence, along with the expression representing the superposition created at that step. In Step 1, we collect a sequence of Q = kn c+1 (n+1)-bit DSP samples Ri = 2− 1/2

P bi∈{ 0,1}| bi, xi + bid⟩ (or, in the case of faulty samples, Ri = | bi, xi⟩ ). For each sample, we Fourier-transform the xi + bid portion, producing the values y1...yQ, and move the yi values to the end, creating the concatenated string Y . The resulting superposition is as follows: 2− (n+1)Q/2

X b1...bQ,Y ωP iyi(xi+bid) | b1, ..., bQ, Y ⟩

In step 2, we compute the sum z =

P

Q i=1 biyi in superposition, and measure all but the highest-order bit of the result, obtaining the (n − 1)-bit

<!-- PDF page 6 -->

value z′. We’ll define Z′Y as the set of vectors ϕ = b1...bQ instantiated by the samples and consistent with Y and the measured value of z′ (for one of h = 0 or h = 1), and h as the highest-order bit of z. The resulting superposition is thus as follows: (X Y | Z′Y| )− 1/2

X Y,ϕ∈ Z′Y (− 1) hdn ωP ixiyi+z′d | ϕ, Y, h, z′⟩

Henceforth we’ll omit the measured value z′ and the constant value ω z′d
from the superposition.
In step 3, we divide the bits bi into groups gj of size c log n, and compute
the value sj as the most significant log n bits of the sum rj =
P gj biyi. The resulting superposition is as follows:
(X Y | Z′Y| )− 1/2

X Y,ϕ∈ Z′Y (− 1) hdn ωP ixiyi | ϕ, Y, h, s1...sQ/c log n⟩

In step 4, we Hadamard-transform the bits bi of each group gj , producing the following superposition over all strings D = b′1...b′Q:

2− Q/2(X Y | Z′Y| )− 1/2

X D X Y,ϕ∈ Z′Y (− 1) hdn+ P bj b′jωP ixiyi | D, Y, h, s1...sQ/c log n⟩

We then measure Y (renaming Z′Y for the measured Y as Z′) and D, and move the measured bits of the first a = n/ log n groups for which the b′i are all 0 to the front (i.e., henceforth labeling them b1...bcn). We call these latter groups (and their associated sj values) set A, and include the remaining β = (Q/c log n) − (n/ log n) groups in set B. We’ll also henceforth omit the now-constant phase coefficient ω xiyi. Lemma 1. If k > c, then with constant probability, the measured value of D = b′1...b′Q will have at least n/ log n groups that consist of all zeroes. Proof. (Sketch) Let D0 be the set of D values with more zeroes than ones, let D1 be the set of D values with more ones than zeroes, and let D0=1 be the remainder (with an equal number of ones and zeroes). Also, let Zeven be the set of ϕ ∈ Z′ with an even number of ones, and Zodd be Z′ − Zeven. The portion of the superposition corresponding to D1 after the measurement of Y is

2− Q/2 | Z′| − 1/2

X D1X ϕ∈ Z′ (− 1) hdn+ϕ· D | D...⟩

= 2− Q/2 | Z′| − 1/2(X D1 X ϕ∈ Zeven (− 1) hdn+ϕ· D | D...⟩ +X D1 X ϕ∈ Zodd (− 1) hdn+ϕ· D | D...⟩ )

<!-- PDF page 7 -->

= 2− Q/2 | Z′| − 1/2(X D0 X ϕ∈ Zeven (− 1) hdn+ϕ· D | D...⟩−X D0 X ϕ∈ Zodd (− 1) hdn+ϕ· D | D...⟩ )

since the phase (− 1) ϕ· D for any D and its counterpart with all the bits reversed will be identical for Zeven, and opposite for Zodd. Given that the sums are all as likely (over choices of Y ) to be negative as positive, and as least as likely to have opposite phases as identical ones between Zeven and Zodd, it follows that the expected total probability (over choices of Y ) of D0 is at least as large as that of D1. Hence with at least constant probability, D has at least as many zeroes as ones. Now, consider the set DY of pairs (D, Y ) such that D ∈ D0 ∪ D0=1 and Y consists of values yi such that the lower-order n − 2 log n bits of the yi values are all distinct. (The latter condition holds for the overwhelming majority of possible Y , and thus DY still covers a constant fraction of the total probability space.) Let DYbad be the set of elements of DY that do not produce the required n/ log n all-zero groups, and DYgood be DY − DYbad. We can map every element of DYbad to a distinct element of DYgood as follows: we first swap locations of zero bits and one bits in D so that the new D is in DYgood, then we swap the lower n − 2 log n bits of the corresponding yi values. The values of ϕ that are in Z′Y will also have their bits rearranged the same way as those of D, and the phase of each such ϕ will be unchanged after the rearrangement, given the rearrangement of the bits of D. The total probability of these mapped elements of DYgood will therefore be as large as the probability of DYbad, and DYgood will thus have an expected probability at least as large as DYbad. It follows that with at least constant probability, the measured value (D, Y ) will be in DYgood. ■ The resulting superposition (assuming the required number of all-zero groups) is as follows:

ν1X ϕ∈ Z′ (− 1) hdn+ P B bj b′j | 0 cnb′cn+1, ..., b′Q, Y, h, s1...sQ/c log n⟩
where ν1 is the normalization coefficient determined by the measurement
of Y and D. Henceforth we’ll omit the measured values Y and (rearranged)
D from the superposition.
We can rewrite this superposition in terms of the sum z∗ =
P A

P rj =

cn i=1 biyi—that is, defining the sets Z′z∗ of states ϕ ∈ Z′ for which

P

cn i=1 biyi =

z∗. We thus create the following superposition:

ν1X z∗ X ϕ∈ Z′z∗ (− 1) hdn+ P Q j=cn+1 bj b′j | h, s1...sQ/c log n⟩

<!-- PDF page 8 -->

Now, consider the set Az∗ of strings ϕA = b1...bcn, ϕA ∈ Z′

P

such that

cn i=1 biyi = z∗, and the set Bz∗,h of strings ϕB = bcn+1...bQ, ϕB ∈ Z′ such that

P

Q i=cn+1 biyi + z∗ = z′ + 2 n− 1h for h = 0 or h = 1. Any ϕ ∈ Z′z∗ must consist of a concatenation of an element of Az∗ and an element of Bz∗,h (for one of h = 0 or h = 1), and moreover any such concatenation must be in Z′z∗ . We can hence rewrite the above superposition as follows:

ν1X z∗,hX ϕA∈ Az∗ X ϕB∈ Bz∗,h (− 1) hdn+ P Q j=cn+1 bj b′j | h, s1...sQ/c log n⟩

In step 5, we measure the values si for groups in B, leaving values S = (σ′1...σ′β). We define the sets Bz∗,S,h as the sets of states in Bz∗,h consistent with the measured value S. The resulting superposition is as follows:

ν2X z∗,hX ϕA∈ Az∗ X ϕB∈ Bz∗,S,h (− 1) hdn+ P Q j=cn+1 bj b′j | h, s1...sa, σ′1...σ′β⟩

where ν2 is the revised normalization coefficient determined by the mea surements. For brevity, we define

Cz∗,S,h = X ϕB∈ Bz∗,S,h (− 1)P Q j=cn+1 bj b′j and rewrite the above superposition as ν2X z∗,hX ϕA∈ Az∗ (− 1) hdn Cz∗,S,h| h, s1...sa, σ′1...σ′β⟩

Henceforth we’ll omit the measured values σ′1...σ′β. In step 6, we compute the sum s∗ =

P j sj (mod n), using s1...sa− 1 and s∗ to erase sa, and compute the bit ls∗ which is 1 if the second-most significant through log log n-most significant bits of s∗ are all 1, and 0 otherwise. We then Hadamard-transform s¯∗ (the low-order log n− 1 bits of s∗), and measure W = (s1...sa− 1, ls∗ ), producing the measured value W′. We proceed only if the measured value of ls∗ is 0 (an event which occurs with at least constant probability, by the same reasoning as Lemma 1). We also measure the Hadamard-transformed bits of s¯∗, and proceed only if the result is all zeroes— an event which occurs with expected probability 2/n, over choices of Y . We define the sets A′z∗,W′ as the sets of states in Az∗ consistent with the measured value W′. We also label the most significant bit of s∗ as h∗, giving us the following superposition:

<!-- PDF page 9 -->

ν3X z∗,h X ϕA∈ Az∗,W′ (− 1) hdn Cz∗,S,h| h, h∗, W′, 0 log n− 1 ⟩ where ν3 is the revised normalization coefficient determined by the mea surements. Note that h∗ as calculated is the most significant bit of the sum of the most significant log n bits of the n/ log n values rj whose sum is z∗, and also that ls∗ = 0 (meaning that the most significant log log n bits of s∗, apart from h∗, aren’t all 1). We can therefore be sure that the omitted less significant bits of the subset sums rj don’t affect the computation of h∗, and hence that h∗ correctly represents the most significant bit of z∗. Henceforth we’ll omit the measured values W′ and 0 log n− 1. Finally, in step 7, we compute and measure h′ = h ⊕ h∗, and use it to erase h. We also replace Cz∗,S,h with Cz∗,S,h′, which is defined equivalently to Cz∗,S,h but with Bz∗ ,S,h replaced by

B′z∗

,S,h′, the same set of states restricted to those elements consistent with the measured value of h′. We thus produce the following superposition:

ν4X z∗ (− 1) (h∗⊕ h′)dn

X ϕA∈ Az∗,W′ Cz∗,S,h′| h∗, h′⟩

where ν4 is the revised normalization coefficient determined by the mea surement of h′. We will now show that

P ϕA∈ Az∗,W′ Cz∗,S,h′ will be approx imately equal for either value of h∗, given two values of z∗ whose least significant n − 1 bits (which we’ll call z¯∗) are equal. Lemma 2. Cz∗,S,h′ is equal for any two values z∗0 and z∗1 of z∗ that differ only in the most significant bit h∗, and either measured value of h′. Proof. For any element ϕB of B′z∗0,S,h′, z∗0 + P

Bbiyi = z′ + 2 n− 1h. We can add or subtract 2 n− 1 as necessary from both z∗0 and z′ + 2 n− 1h to obtain the equality z∗1 +

P

Bbiyi = z′ + 2 n− 1h for the other value of h∗ (and hence of h, since we’ve fixed h′ = h∗ ⊕ h by measuring it). Thus B′z∗0,S,h′ = B′z∗1,S,h′, and the lemma’s claim follows. ■ We can therefore rewrite the above superposition in terms of Cz¯∗ = Cz∗,S,h′ for either z∗ whose least significant n − 1 bits are z¯∗ (and the measured value of h′), as follows:

ψ = ν4X z∗ (− 1) (h∗⊕ h′)dn

X ϕA∈ Az∗,W′ Cz¯∗ | h∗, h′⟩ We next show that the portion of the amplitude of either value of h∗ that is associated with a particular z∗ is of bounded size. This will allow

<!-- PDF page 10 -->

us later to convert the bounded relative deviations from uniformity of these amplitude portions into a bounded absolute total deviation across all values of z∗. Definition 1. For a given measured z′, and a resulting superposition ψ of values of h∗ at the end of step 7, let M be the set of values (Y, D, W′, S, h′) measured (apart from z′) during the algorithm, let T + M (resp., T−M) be the set of states ϕ consistent with M and h∗ and with phase 1 (resp., − 1) resulting from measurement of M (ignoring the constant phase ω yz′d). Let TM = T + M ∪ T−M, and let tM (resp., t + M, t−M) be | TM| (resp., | T + M| , | T−M| ). Then ψ is well-behaved for a value of h∗ if | t + M − t−M| ≥ Ω(2− n/2√tM). Lemma 3. With probability 1 − O(2− n) (over choices of M), the super position ψ at the end of step 7 is well-behaved for at least one value of h∗. Moreover, if it’s well-behaved for a particular h∗, then the probability (over choices of M) that for some z∗ consistent with that value of h∗ the amplitude αz∗ = ν4

P ϕA∈ Az∗,W′ Cz¯∗ has magnitude greater than Ω(2 3n/2) is less than O(2− n). Proof. (Sketch) Let TM′ (resp., T + M′, T−M′) be defined identically to TM (resp., T + M, T−M), but with h¯′ replacing h′, and let T = TM ∪ TM′, T + = T + M ∪ T + M′, and T− = T−M ∪ T−M′. Note that TM and TM′ are disjoint and of equal expected size t/2 (where t = | T| ) over choices of Y , since for any Y that maps a given ϕ to TM, there’s a corresponding Y′ that maps it to TM′, where the difference is only the addition of 2 n− 1 to two yi values, one on either side of the boundary between ϕA and ϕB—say, in ga on the ϕA side, and among the unused samples on the ϕB side, to maintain consistency with measured si values. Note that this partition depends only on Y , and not on D. (All values of D are equally compatible with any partition of ϕ values into TM and TM′—D only determines the ϕ values’ phases.) Also, for any set MD of M values with fixed (Y, W′, S, h′) and varying D, any ϕ (resp., any pair ϕ1 and ϕ2), regardless of whether in TM or TM′, is equally often in T + or T− (resp., both in T +, both in T−, or one of them in each), over choices of D. (This holds as long as there’s a single non-zero bit in ϕ, since that bit leaves the phase of ϕ unchanged if paired with a zero in D—that is, for half the possible values of D—and flips ϕ’s phase if paired with a 1 in D. The exception, the all-zeroes state, only appears in the superposition if the measured value of z was 0, which occurs with exponentially small probability.) The phases of the elements of TM can thus be considered to be selected pairwise-independently over choices of D. The sizes of T + M and T−M—that is, t + M and t−M—will therefore have mean and variance σ 2 M = Θ(tM). Now, let αϕ be the size of the amplitude of

<!-- PDF page 11 -->

a single state ϕ for a value of M in a particular MD. (αϕ will be the same for any M in a particular MD, since the measured values apart from D are fixed, and D doesn’t affect which states ϕ contribute to M—only the phase of their contributions.) Then the expected probability of M, E(α 2 ϕ(| t + M − t−M| ) 2) = E(α 2 ϕ(t + M − t−M) 2), is proportional to the variance σ 2 M of t + M − t−M, since (E(t + M − t−M)) 2 = 0. Moreover, the values of M for which | t + M − t−M| ≤ O(2− n/2√tM) for both values of h∗—i.e., those for which ψ is not well-behaved—also have probability α 2 ϕ| t + M − t−M|

2 ≤ O(2− nα 2 ϕtM) = O(2− nE(α 2 ϕ(t + M − t−M) 2)). Their combined probability—i.e., the probability that ψ is not well-behaved for both values of h∗—is thus bounded above by O(2− n | MD| E(α 2 ϕ(t + M − t−M) 2))) ≤ O(2− n)P r[MD], since | MD| EM∈ MD(P r[M]) = P r[MD]. Hence the total probability that ψ is not well-behaved for both values of h∗ is at most O(2− n). Similarly, if we assume a well-behaved ψ for a particular value of h∗, then for each z∗ consistent with that h∗ the phases of the elements of subsets TM,z∗ of TM (restricted to states ϕ compatible with z∗, and with size tM,z∗ bounded above by tM) can again be considered each selected pairwise independently, resulting in t + M,z∗ and t−M,z∗ (for the natural definitions of t + M,z∗ and t−M,z∗ )

both having standard deviations at most O(q t + M) and O(q t−M), respectively. Thus by Chebyshev’s inequality, the probability that | t + M,z∗ − t−M,z∗ | is greater than O(2 n√tM) is at most O(2− 2n). And since in a well-behaved ψ for a particular h∗ the total | t + M − t−M| of states contributing to the amplitude of h∗ is at least O(2− n/2√tM)—i.e., the sum of at least O(2− n/2√tM) states is required for αz∗ to produce an amplitude of O(1)—we can conclude that αz∗ is greater than O(2 3n/2) with probability at most O(2− 2n). Hence the probability that αz∗ ≥ O(2 3n/2) for some z∗ is at most O(2− n). ■ Corollary. For any log n-bit value z∗h let Z∗h be the set of values of z∗ whose most significant log n bits are z∗h, and let αz∗h be the amplitude of z∗h in the superposition, i.e.,

P Z∗h αz∗ , where αz∗ is defined as in Lemma 3. We say that ψ is very well-behaved for a value of h∗ if | t + M − t−M| ≥ Ω(p tM/n). Then with probability 1 − O(1/n) over choices of M, ψ is very well-behaved for at least one h∗, and moreover if it’s very well-behaved for a particular h∗, then the probability (over choices of M) that for some z∗h consistent with h∗ the amplitude αz∗h has magnitude greater than n 3/2 is less than O(1/n). Proof. (Sketch) The proof follows the structure of Lemma 3, but with z∗h replacing z∗ and n substituting for 2 n (and hence √n substituting for 2 n/2 and n 3/2 for 2 3n/2). ■ Next, we will show that for two z∗h values that differ only in h∗, their

<!-- PDF page 12 -->

implicit amplitudes in ψ following step 7 are approximately equal. We do so by demonstrating that the ϕA state portions are so close to uniformly distributed over z∗ values that, given the bound established by Lemma 3, the amplitudes of any two z∗ values differing only in h∗ are close to identical—so close, in fact, that their sums across all z∗ values sharing the same z∗h (apart from h∗) are also approximately identical. Lemma 4. For any z∗, let z∗h be the most significant log n bits of z∗, and z∗l be the remaining bits. Then for any two values z∗h,0 and z∗h,1 of z∗h that differ only in the most significant bit h∗, and for c ≥ 12, the amplitudes αzh,0 and αzh,1 (defined as in the corollary to Lemma 3) differ by an expected multiplicative factor 1 ± O(n− (((c− 1)/2)− 1)) with probability at least 1 − 1/n. Proof. (Sketch) For any ϕA, the value of z∗h is the sum of the “overflow” bits produced by summing the relevant z∗l values, the measured sj values, and sa. We will focus on the latter, dividing the state-portions ϕA into sets Aga, each with a different value of the string qga of values of the bits in ga, and assuming that z∗h is fully determined by sa. (Considering the additional variation from the “overflow” bits in determining z∗h would only increase the uniformity of z∗h, which we want to maximize in any event.) Note that the sets Aga are of equal size, since a particular value of qga doesn’t restrict the values of the rest of the bits in ϕA, as long as z∗ isn’t specified. Since the values of sa for ϕA in different Aga are pairwise independent over all possible yi values, we can model the distribution of z∗h values—based solely on the variation in sa—as a “balls in bins” problem, with n c balls and n bins. The mean for each bin is thus n c− 1 balls, with a variance n c− 1 and standard deviation n (c− 1)/2 for each bin. (We are assuming here that ga contains no faulty samples—an event which occurs with probability greater than (1 − 1/c′log n) c log n = e− c/c′.) By Chebyshev’s inequality, then, the probability that the number of balls in a particular bin differs from n c− 1 by κn (c− 1)/2 is at most 1/κ 2, and the probability that any bin deviates from the mean by κ′n (c− 1)/2 is thus at most n/κ′ 2. For example, for κ = n, the bins deviate from the mean by at most n ((c− 1)/2)+1 except with probability at most 1/n. The values of z∗l for different ϕA are also pairwise independent, so we can again model the distribution of z∗l values for each Aga as a “balls in bins” problem, with an expected τ = 2 cn(1− 1/(c′log n))− n− c log n balls (since the n − log n measured bits of W′, plus the log n bits of z∗h, reduce the cn bits of A by an expected n effective bits, and ga has c log n bits) dis tributed among 2 n− log n bins. (To simplify the calculations we’ll consider the number of bins as 2 n.) The mean number of balls per bin is thus

<!-- PDF page 13 -->

µ = 2 cn(1− 1/(c′log n))− 2n− c log n, with an equivalent variance and thus a stan dard deviation σ = 2 (cn/2)(1− 1/(c′log n))− n− c log n/2. By Chebyshev’s inequality, then, the number of balls will deviate from µ by κ′σ for a given bin with probability at most 1/κ′ 2 . Setting κ′ = 2 n and c ≥ 12, we get that with probability at least 1 − 2− 2n, a bin deviates from µ by at most 2 (12n/2)(1− 1/(c′log n))+n− 12 log n/2 balls—or, in amplitude terms, a z∗l value deviates by that much from the expected number of ϕA state portions. We know from Lemma 3 that the amplitude of a given (z∗h, z∗l) pair such that ψ is well-behaved for the high-order bit of z∗h has (except with exponentially small probability) magnitude at most 2 3n/2, and since the expression αz∗ = ν4

P ϕA∈ Az∗,W′ Cz¯∗ for the amplitude of the pair (z∗h, z∗l) changes only in the number of ϕA state portions in the sum when the h∗ value is changed—and the numbers of ϕA state portions for both values of h∗ are within a small range around the expected number—the same approximate bound applies for both values of h∗. It follows that with probability at least 1 − 2− n, the total deviation δ from µ over all 2 n − n possible values of z∗l for a given z∗h is at most
((2 (12n/2)(1− 1/(c′log n))+n− 12 log n/2)/µ)(2 n)(2 3n/2)
= (2 (12n/2)(1− 1/(c′log n))+7n/2− 12 log n/2)/µ
< 2− n
even if all the relative deviations are maximal and all in the same direction.
We further observe from the superposition ψ yielded by step 7 that the
amplitude associated with a given set Aga is given by

ν4X z∗l (− 1) (h∗⊕ h′)dn

X ϕA∈ A(z∗h ,z∗l ),W′∩ Aga C ¯ (z∗h,z∗l )

Because of the nearly-uniform distribution of the ϕA state-portions over values of z∗l, though, we can treat this amplitude as a simple sum of the relevant Cz¯∗ values, multiplied by the expected number µ of compatible state-portions ϕA, ignoring the resulting exponentially small error δ. That is, we can approximate the amplitude of Aga as

ν4(− 1) (h∗⊕ h′)dn | Aga|X z∗l C ¯ (z∗h,z∗l )

where the sets Aga are of equal size. Moreover, for two qga values whose corresponding z∗h values differ at most in h∗ the Cz¯∗ corresponding to each

<!-- PDF page 14 -->

z∗l is the same (by Lemma 2)—as is, therefore, the sum over all z∗l of the

Cz¯∗ . Hence if we define J = ν4| Aga|P z∗l C ¯ (z∗h,z∗l ) for a given z∗h, then the amplitude of z∗h is just J multiplied by the number of sets Aga corresponding to the given z∗h. The relative multiplicative difference between two values of z∗h differing only in h∗ is thus bounded (with probability at least 1 − 1/n) by 1 ± O(n ((c− 1)/2)+1− (c− 1)) = 1 ± O(n− (((c− 1)/2)− 1)), as required. ■ Using Lemma 4 along with the corollary to Lemma 3, it follows that the amplitudes of the values of z∗h in the superposition ψ following step 7 are all at most n except with probability at most O(1/n), and hence for c ≥ 12 the sum of the differences in amplitudes across the n/2 pairs z∗h that differ only in h∗ is at most O(n− (((c− 1)/2)− 3)) ≤ 1/n except with probability 1/n. This is small enough compared to the approximately 1/√2 amplitude of at least one of the two possible values of h∗ (and hence both, since their difference is so small) that Hadamard-transforming and measuring h∗, which has inherited its encoding of dn from h by the computation of h′, will yield dn with probability significantly greater than 1/2. Thus polynomially many iterations of the algorithm will allow us to extract dn with near-certainty.■ Corollary. There exists a polynomial-time quantum algorithm that com putes O(√n polylog(n))-factor approximations of SVP solutions, and solu tions to LWE instances with α = O(√n polylog(n)).[9][7]

## 3 Acknowledgements

The author would like to thank Sanketh Menda, Daniele Micciancio, Seyoon Ragavan, Vinod Vaikuntanathan and Thomas Vidick for many immensely useful discussions and suggestions.

## References

[1] M. Ajtai and C. Dwork. A Public-Key Cryptosystem with Worst Case/Average Case Equivalence. In In STOC, pages 284–293. ACM Press, 1997.

[2] D. Boneh and R. Lipton. Quantum cryptanalysis of hidden linear forms. In CRYPTO, pages 424–437, 1995.

[3] Z. Brakerski, E. Kirshanova, D. Stehl´e, and W. Wen. Learning with Errors and Extrapolated Dihedral Cosets. In PKC 2018, pages 702–727, 2018.

<!-- PDF page 15 -->

[4] Y. Chen and P. Q. Nguyen. BKZ 2.0: Better lattice security estimates. In ASIACRYPT, pages 1–20, 2011.

[5] M. Ettinger and P. Høyer. On quantum algorithms for noncommutative hidden subgroups. Adv. in Appl. Math., 25(3):239–251, 2000.

[6] G. Kuperberg. A subexponential-time quantum algorithm for the dihe dral hidden subgroup problem. SIAM J. on Computing, 35(1):170–188, 2005.

[7] D. Micciancio. Personal Communication.

[8] C. Peikert. A Decade of Lattice Cryptography. Foundations and Trends in Theoretical Computer Science, 10(4):283–424, 2016.

[9] S. Ragavan. Personal Communication.

[10] O. Regev. Quantum Computation and Lattice Problems. SIAM J. on Computing, 33(3):738–760, 2004.

[11] O. Regev. On Lattices, Learning with Errors, Random Linear Codes, and Cryptography. In In STOC, pages 84–93. ACM Press, 2005.

[12] P. Shor. Polynomial-time algorithms for prime factorization and dis crete logarithms on a quantum computer. SIAM J. on Computing, 26(5):1484–1509, 1997.

<!-- PDF page 16 -->
