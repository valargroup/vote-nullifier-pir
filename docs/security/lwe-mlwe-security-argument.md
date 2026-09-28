---
title: "On the Concrete Hardness Gap Between MLWE and LWE"
author: "Tabitha Ogilvie"
year: 2026
eprint: "2026/279"
source_pdf: "lwe-mlwe-security-argument.pdf"
source_url: "https://eprint.iacr.org/2026/279"
license: "CC BY"
format: "AI-readable Markdown transcription"
---

# On the Concrete Hardness Gap Between MLWE and LWE

**Author:** Tabitha Ogilvie  
**Affiliations:** Royal Holloway, University of London; King’s College London  
**Contact:** `tabitha.l.ogilvie@gmail.com`

> Conversion note: This is a structured text transcription of the 48-page PDF. Section hierarchy, algorithms, tables, references, mathematical symbols, and extractable page boundaries are retained as text. PDF extraction can flatten typography in dense equations and matrices; consult the linked source PDF when exact typesetting is security-critical.

## AI navigation note

_This note is editorial metadata, not part of the authored paper._

- **Central claim:** Concrete LWE estimates can overstate the hardness of MLWE/RLWE because ring symmetries strengthen hybrid attacks.
- **Mechanism:** A coefficient isometry multiplies an MLWE target by a signed coefficient permutation while leaving the public matrix and relevant secret/error distributions unchanged. Expensive preprocessing can therefore be reused across more guesses.
- **Power-of-two cyclotomics:** Multiplication by each monomial `X^j` is a negacyclic coefficient rotation and hence a coefficient isometry.
- **Reported impact:** Up to 15 bits for sparse-secret RLWE and roughly 2–3 bits for Kyber/ML-KEM under the paper’s stated cost models.
- **Primary formal results:** Definitions 10–14; Lemmas 11–25; Algorithms 4–7.
- **Exact-notation warning:** Use the PDF for any equation or matrix where superscript/subscript placement affects an implementation or proof.

## Abstract

Concrete security estimates for Module-LWE (MLWE) over an appropriate ring are often obtained by translating to an “equivalent” unstructured LWE instance, which implicitly treats algebraic structure as a pure efficiency gain with no impact on security. We show that this heuristic fails for realistic parameters. In common MLWE/RLWE instantiations, an attacker can exploit symmetries to obtain hybrid attacks that are strictly stronger than the best corresponding attack on LWE, translating to a concrete hardness gap between MLWE and LWE.

Our starting point is the observation that many cryptographically relevant rings admit coefficient isometries: ring elements whose multiplication acts as a signed permutation on coefficient vectors and preserves the secret and error distributions of interest. Multiplying an MLWE instance by such an isometry creates many derived instances that share the same public matrix and are therefore compatible with the same expensive offline preprocessing in hybrid attacks. We formalise this mechanism and incorporate it into both primal and dual hybrid frameworks.

We instantiate coefficient isometries for power-of-two cyclotomic rings, and quantify the resulting advantage in two regimes. For sparse-secret RLWE (popular in homomorphic encryption), isometry-enabled hybrids yield gaps of up to 15 bits over LWE-based estimates. For the standardised Kyber/ML-KEM parameters, we obtain a consistent ≈ 2–3 bit gap under standard cost models. Our results demonstrate that the widely assumed equivalence between LWE and MLWE in power-of-two cyclotomics does not hold, with real world consequences for deployed schemes.

## 1 Introduction

### 1.1 Motivation

The Learning with Errors (LWE) [66] problem and its structured variant the Module Learning with Errors (MLWE) [45,49,50] problem have proven to be a fruitful foundation for many cryptographic primitives, including digital sig natures [34], key exchange [10,17], and fully homomorphic encryption [40,20]. MLWE in particular has led to many practical constructions, owing to its effi ciency: at a high level, the module structure enables a lower ciphertext expansion

<!-- PDF page 1 -->

factor when compared to LWE, decreasing bandwidth and computation require ments [48]. Concrete security estimates for MLWE-based schemes are frequently ob tained by relating MLWE to an unstructured LWE instance [1,18]. Implicitly, this assumes the additional algebraic structure affords no additional advantage to an attacker. Because many deployed parameters rely on this assumption, un derstanding its validity has immediate practical consequences. In this work, we show that this assumption is not true in general: for realistic cryptographic pa rameters, an attacker can exploit the ring structure to get an attack which is strictly better than the best attack on the corresponding LWE instance. In other words, we find a concrete gap between the hardness of MLWE and LWE.

### 1.2 Main Idea
Our results come from a simple observation: in many cryptographically relevant
MLWE parameterisations, the underlying ring/module admits symmetries that
preserve the distributions of secrets and errors, while reindexing the secret’s
coefficients. These symmetries can be exploited algorithmically to strengthen
hybrid attacks.
In more detail, consider a single MLWE sample (a, b) over a ring R q,

b = ⟨ a, s⟩ + e (mod R q),

For any r ∈ R q, we can multiply the equation by r to obtain

rb = ⟨ a, rs⟩ + re (mod R q).

This transformation is always algebraically valid; the question is whether this is still distributed like an MLWE sample with the same parameters. In many cryptographic settings, there exist nontrivial choices of r where multiplying by r acts as a signed permutation on coefficient vectors, and so preserves the relevant norms and (crucially) the secret and error distributions. Such an r produces a new MLWE sample (a, rb) that has the same a, but now with the permuted secret rs. The consequence for hybrid attacks is that any expensive preprocessing de pending only on a can be used to find any secret rs: in practice, this additional freedom means we can find the secret faster. An important special case arises in the rings in deployed and standard ised MLWE schemes, which typically use power-of-two cyclotomic rings R q = Zq[X]/(X n + 1). Here, multiplication by monomials X j corresponds to a nega cyclic rotation of coefficients, giving a large family of such symmetries. We quantify the impact of these symmetries in two complementary regimes. First, we study sparse-secret Ring Learning with Errors (RLWE) (ubiquitous in Homomorphic Encryption), where hybrid attacks are already the most perfor mant attack, and where our symmetry technique has the largest effect: We find up to a 15 bit gap between LWE and RLWE hardness in this setting, downgrad ing the security of many recent publications below their target level. Second,

<!-- PDF page 2 -->

we study the standardised Kyber/ML-KEM parameters and find that exploit ing cyclotomic symmetries gives a consistent 2 − 3 bit gap compared to LWE estimates.

### 1.3 Technical Overview
We now give a more technical overview of our work.
Hybrid Algorithms In Algorithm 1, we give the outline for a generic hybrid
guessing algorithm, which captures both the primal and dual hybrid attacks on
LWE and MLWE. We see that this algorithm has two phases, starting with some
offline computation, before looping through a search space S. If our guesses are
for some secret s, we can calculate the time complexity of this attack as

1 Pr[s ∈ S] TInitialWork + | S| TCheck

so that the best possible attack involves carefully balancing the size and hitting probability of | S| , as well as the amount of initial and per guess computation.

Algorithm 1 Generic Hybrid Guessing Algorithm
1: Input: A set of guesses S
2: Output: A successful guess g or
⊥3: W ← InitialWork()
4: for all g ∈ S do
5: result ← Check(W, g)
6: if result is successful then
7: Return g
8: Return ⊥

The key constraint in Algorithm 1 is that the initial work must be compatible with each guess g; we must have that the offline computation actually enables successfully checking whether g is correct. In hybrid attacks on LWE, this pre computation concerns a fixed part of the public matrix A: all guesses must be compatible with the same fixed part of A. Coefficient Isometries In order to specify when the ring structure ensures differ ent guess sets S are possible, we define the notion of coefficient isometries. These are ring elements r ∈ R that ensure that whenever (A, b = As+e) ∈ R m× (k+1) q is an MLWE sample, (A, rb = A(rs)+re) is also an MLWE sample, with identi cal parameters. This implies that if an attack succeeds for (A, b), it will equally succeed for (A, rb). Crucially, any precomputation with respect to A can be used equally on (A, b) to find s and (A, rb) to find rs. We define coefficient isometries in such a way that they preserve all secret and error distributions we consider.

<!-- PDF page 3 -->

Impact on Primal and Dual Hybrids Coefficient isometries produce derived in stances (A, rb) that share the same public matrix A but correspond to a per muted secret rs. Since hybrid attacks amortise expensive preprocessing that depends only on a fixed part of A, these derived instances enlarge the family of guesses that are compatible with the same offline work. We formalise this effect for both primal and dual hybrid frameworks in Sections 3 and 4.

Power-of-Two Cyclotomics We now instantiate coefficient isometries in the most common rings used by deployed MLWE schemes, namely power-of-two cyclo tomics R q = Zq[X]/(X n + 1) for some power-of-two n ∈ N. In these rings, multiplication by X j corresponds to applying a signed permutation matrix: this means that every monomial X j is a coefficient isometry according to our defini tion. We quantify the power of these isometries in the power-of-two cyclotomic case for both Primal and Dual Hybrid attacks by plotting the empirical hit probability Pr[s ∈ S] against | S| for the LWE (fixed-coordinate) guessing strategy and for our isometry-enabled strategy, and we observe a consistent advantage.

### 1.4 Contributions
This paper shows that the algebraic structure in deployed MLWE instantiations
enables strictly stronger attacks than those available in the corresponding un
structured LWE setting. Concretely, our contributions are:
1. Coefficient isometries as a mechanism for stronger hybrids. We iden tify and formalise the symmetries that matter for concrete hybrid attacks, calling these coefficient isometries, i.e. ring elements whose multiplication acts as a signed permutation on coefficient vectors. For secret and error distri butions invariant under these isometries, multiplying an MLWE instance by an isometry preserves all parameters while reindexing the secret, producing additional instances that are compatible with the same offline preprocessing.
2. Primal hybrid with isometries. We give an MLWE adaptation of the primal hybrid attack (IsometricPrimalHybrid) that exploits coefficient isometries. We prove that the corresponding runtime and success probability analysis is unchanged for isometry-invariant secret and error distributions, so the effect of isometries is isolated to the Pr[s ∈ S] vs. | S| tradeoff. We ad ditionally propose a meet-in-the-middle variant under a natural unbalanced decomposition heuristic.
3. Dual hybrid with isometries. Building on the dual hybrid attack of [23], we describe IsometricDualHybrid, which samples an isometry per trial and tests a zero pattern event on the permuted secret. We prove that, under isometry-invariant distributions, the distinguishing analysis is unchanged, so any concrete gain again comes through an increased hit probability η.
4. Instantiation and advantage in power-of-two cyclotomics. For R q = Zq[X]/(X n + 1) with n a power-of-two, we show every monomial X j is a

<!-- PDF page 4 -->

coefficient isometry (negacyclic rotations), leading to algorithms RotPri malHybrid and RotDualHybrid. For fixed Hamming weight and Kyber secrets, we derive accurate approximations to the resulting hit probabili ties which we empirically validate for cryptographically relevant parameters. These experiments additionally demonstrate a consistent improvement in the Pr[s ∈ S] vs. | S| tradeoff compared to the LWE guessing strategy. 5. Concrete security consequences. Using these attacks, we quantify a con crete gap between MLWE/RLWE and the corresponding unstructured LWE estimates in realistic regimes: up to a 15-bit gap for sparse-secret RLWE, and a consistent ≈ 2–3 bit gap for the Kyber/ML-KEM parameter sets under the same reduction-cost models as prior work.

### 1.5 Related Work

Concrete hardness of LWE and hybrid attacks. Understanding the concrete complexity of LWE attacks has been the subject of extensive work, including [7,3,51,6,14,47,22]. In regimes where the secret is drawn from a small or struc tured distribution, hybrid approaches that combine lattice reduction with guess ing are often the most effective, as demonstrated in [2,4,69,27,38]. Our primal attack is an MLWE adaptation of the primal hybrid as modelled by the Lattice Estimator [7]. Structured LWE and LWE Concrete Hardness. For power-of-two cyclotomics, concrete security estimates for RLWE/MLWE are commonly obtained by con sidering an “equivalent” unstructured LWE instance, treating the ring/module structure as an efficiency feature with no security impact. This is implicit in the current Homomorphic Encryption standard [18] and was made explicit in the previous standard [1]. Beyond power-of-two cyclotomics, a recent line of work [33] examines the av erage case performance of the module BKZ algorithm [46,57], and find a sublinear gain on the required blocksize in non power-of-two cyclotomics. This represents a different attack style to our work, where we use the ring structure to enhance hybrid guessing strategies. Moreover, our algorithms can be used even on RLWE instances. We use the non-dual form for MLWE, which resembles the Polynomial LWE (PLWE) problem [67]. Using the non-dual form can leave MLWE vulnerable to (polynomial time) attacks in rings for which the error distribution is signifi cantly distorted when mapping from the ring to its dual [62,36,37,24]. As such, equivalence between non-dual MLWE and LWE is never assumed in such rings. Exploiting Symmetries in Lattice Cryptanalysis. Conceptually, our work is re lated to attacks on NTRU based schemes [59,35], particularly to the zero forc ing technique for the NTRU problem [53]. Here, a dimension reduction can be achieved by observing that any rotation by a power of X of the secret polynomial vector is a small vector in the lattice, so it is enough to search for any rotation where a chosen set of coordinates is zero. However, as MLWE is inhomogeneous

<!-- PDF page 5 -->

and NTRU is homogeneous, in our setting we must commit to both a rotation and a guess for a set of coordinates: a correct drop set but incorrect rotation gives no information. This necessitates a different analysis. Additionally, the zero-forcing technique doesn’t constitute a hybrid attack. Indeed, if the guessed zero pattern matches no rotation, the attack must start again: the expensive reduction cannot be amortised across many guesses. Rotations were also recently used in the Cool & Cruel attack on sparse RLWE over power-of-two cyclotomics [60,70]. There, one performs full-dimensional lat tice reduction and relies on obtaining a particular “Z-shaped” reduced basis pro file, which induces a decomposition into a “cruel” part (handled by brute force) and a “cool” part (recovered over the integers). Rotations are then used to shift which secret coefficients align with the “cruel” portion, enabling a parallelised brute-force over rotations. Our work differs from Cool and Cruel in both struc ture and applicability. We build on the primal and dual hybrid attacks, both of which perform lattice reduction in a reduced dimension, leading to more efficient attacks in practice. In addition, the Cool and Cruel attack depends critically on obtaining a Z-shaped basis, a shape which may disappear as parameters get larger, especially as the corresponding Z-shape in the GSO profile disappears as q gets large [63]. Moreover, in our attack, the guessing dimension is indepen dent of the of the reduced basis shape, whereas in Cool and Cruel it is tied to the width of the Z-shape “cliff". These differences make our attack structurally different and better aligned with standard estimation frameworks. We give an overview of the differences between our proposed algorithms and [60] in Table 4 in Appendix A.

Code based dual-hybrid attack. For the Kyber/ML-KEM parameter regime, our dual attack builds directly on the code-based dual hybrid attack of [23]. This work introduced a decoding approach to make enumeration over residual secret key coefficients feasible, along with properly analysing the true positive and false positive probabilities of the resulting attack.

Paper Outline. In Section 2 we recall the primal and dual hybrid attacks on LWE. In Section 3 we introduce coefficient isometries and adapt the primal hybrid attack to MLWE. In Section 4 we give the corresponding adaptation of the dual hybrid attack. In Section 5 we instantiate these attacks in power-of-two cyclotomics and derive concrete security estimates for sparse-secret RLWE and for the Kyber/ML-KEM parameter sets. Additional details are deferred to the appendices.

## 2 Preliminaries

### 2.1 Notation

We write log(· ) for base-2 logarithms and ln(· ) for natural logarithms. Vectors are denoted by bold lowercase letters and matrices by bold uppercase letters.

<!-- PDF page 6 -->

We view vectors as columns and write ⟨· , ·⟩ (or · ) for the standard dot product.
We write ∥·∥ for the Euclidean norm.
We identify Zq with the interval [− q/2, . . . , q/2) ∩ Z, and write mod q for
reduction into this set. We write x

$ ← S for uniform sampling from a finite set

S, and x

d = y for equality in distribution. For a ring element x ∈ R , let coeff(x) denote its coefficient vector. For x = (x1, . . . , xk) ∈ R k, write (x)coeff := (coeff(x1) | · · · | coeff(xk))

for the flattened coefficient vector.

### 2.2 Lattices

We detail necessary lattice background in Appendix B.1.

### 2.3 Learning with Errors, Module Learning with Errors, and Ring Learning with Errors
Our work is concerned with the Learning with Errors (LWE), Module Learning
with Errors (MLWE), and Ring Learning with Errors (RLWE) problems defined
as follows, following [18].
Definition 1 (LWE Distribution). For a secret s ∈ Z n q that is chosen accord
ing to χs, the LWE distribution samples a ∈ Z n q uniformly at random, samples
e ∈ Zq according to χe, and outputs (a, b := a · s + e mod q) ∈ Z n+1 q .
Definition 2 (Decision LWE). The Decision LWE problem is to distinguish
LWE samples (a, b) from uniform.
Definition 3 (Search LWE). The Search LWE problem is to recover the secret
vector s given m samples from the LWE distribution.
It will be convenient to collect all the LWE samples into a single matrix
vector equation, writing b = As + e mod q, where each row is a different LWE
sample.
If we replace Z by a ring R , we obtain the Module Learning with Errors
(MLWE) problem over R q := R /qR . In this setting the secret and error are
sampled from distributions χs and χe over the appropriate R q-modules.

Definition 4 (MLWE Distribution). For a secret s ∈ R

k q drawn from χs, the MLWE distribution samples a

$ ← R

k q, samples e ← χe in R q, and outputs (a, b := a · s + e mod q) ∈ R

k q × R q. Definition 5 (Decision MLWE). The Decision MLWE problem is to distin guish MLWE samples (a, b) from uniform.

<!-- PDF page 7 -->

Definition 6 (Search MLWE). The Search MLWE problem is to recover the secret vector s given m samples from the MLWE distribution. Once again it will be convenient to stack m such samples into a single matrix vector equation, this time over R q, writing b = As + e ∈ R

m q , where each row

is a different MLWE sample.
The special case k = 1 gives the Ring Learning with Errors (RLWE) problem.
Following prior work estimating the concrete hardness of these problems [7,3],
we treat the cost of search and decision as comparable, due to the tight reductions
which exist between them [65,54,21].

### 2.4 The Primal Hybrid Attack On LWE

We now give an overview of the Primal Hybrid attack, which we build on directly. For more details on this algorithm we refer to [4,69,71]. This algorithm uses Babai’s Nearest Plane algorithm [12] to find close vec tors: 1 we detail this algorithm and corresponding probabilities in Appendix B.2. From this appendix, we use the notation NP(· ) to denote calling this algorithm; pbabai for the probability this algorithm succeeds; and pmitm for the probability the algorithm achieves additive homomorphism for a given displacement. We fully specify the Primal Hybrid algorithm, short secret version, in Algo rithm 2, synthesising [4,69,71]. At a high level, this algorithm starts by perform ing a lattice reduction which makes finding close vectors easier, then calls NP for various guesses at a chunk of the secret sζ . We can relate this to the generic hybrid guessing algorithm characterisation given in Algorithm 1 of the intro duction by understanding InitialWork() as the BKZ reduction, and Check() consisting of the NP call.

#### 2.4.1 Runtime and Correctness

For a guessing set S, our runtime consists
of an initial BKZ reduction, followed by one call to NP for each guess sg. This
leads us to the following runtime.
Lemma 1 (Primal Hybrid, no MitM, Runtime). The runtime of Algo
rithm 2 is given by

TBKZ(β, d) + | S| TNP(d)

Various cost models exist for the runtime of BKZ: in general, the cost is exponen tial in the blocksize β. We leave the precise cost model for finding the shortest vector (which translates to a cost model for BKZ) as a free parameter in our estimator. The Primal Hybrid attack will succeed provided the guessing set contains the correct sζ , and NP is able to correctly recover (e, − ξsn− ζ ). 1 An alternative subroutine is measured by the Lattice Estimator. This procedure projects into a lower rank and uses an exact shortest vector oracle in this lower rank to find the projection of the LWE error and secret. However, this subroutine does not seem to admit a meet-in-the-middle speedup, and is therefore not competitive in the parameter regimes we consider.

<!-- PDF page 8 -->

| Algorithm | 2 | PrimalHybrid | |
| --- | --- | --- | --- |
| Input: – – – – – Output: ⊥1: Select columns 2: Construct 3: B′BKZ 4: for 5: 6: 7: 8: 9: return | Samples oracle guessing guessing LWE error BKZ blocksize a short ζ random form the BKZβ(BBKZ) ← sg each ∈ (b t ← (e, ξsn− − (sg, if sn− return ⊥ | (A, b = dimension S set ⊆ variance β (s, e) columns ζ An− ∈ BKZ basis BBKZ S do sg) Aζ − 0 T ) ζ NPB′BKZ ← , e) ζ is (sg, sn− | m n m× LWE(q, n, m, χs, χe) As + e) produced by an q q Z Z × ∈ ζ n ≤ ζ q Z ξ = σe σe/σs and secret scaling factor As + e = b (mod q) with or ζ m× Aζ of A; call the sub–matrix . The remaining q Z ∈ ζ) m× (n− q . Z qIm ζ d An− d× = , d m + n ζ. R 0 ξI ζ ∈ ← − n− mod q (t) a valid secret then , e) ζ |

Lemma 2 (Primal Hybrid, no MitM, Success Probability). Algorithm 2 succeeds with probability

pNP · Pr [sζ ∈ S] , where pNP is determined by Lemma 28 applied to displacement (e, − ξsn− ζ ) and basis B′BKZ. Proof. Given in Section B.3. We combine these into a single security estimate following the bit security frame work [55]: namely, considering the lowest possible value for runtime divided by success probability across all parametrisations of this algorithm.

#### 2.4.2 Meet in the Middle Speedup

It is possible to achieve a meet-in-the middle speedup during the guessing phase, allowing us to reduce the number of calls to the NP algorithm. When we use this speedup, we have an additional success probability pmitm, as discussed in in Appendix B.3.1. To the best of our understanding, standard practice is to conservatively assume it is possible to reduce the number of NP calls from | S| to exactly p | S| , which requires the following heuristic.

<!-- PDF page 9 -->

Heuristic 1 (Plain S Decomposition.) There is an additive decomposition of S that means we require p | S| NP calls. This heuristic also appears in ongoing work on HE standardisation [31]. Al ternative approaches are discussed in [69,71,52]. For example, Son and Cheon [69] consider building the sets S1 and S2 and then defining the guessing set S as their sum; Wunderer [71] considers a decomposition of the set of Hamming weight hg guesses into two set of Hamming weight hg/2, which asymptotically is a square root speedup; May [52] discusses a method of splitting the guess vectors into two vectors of half the length, and splitting the Hamming weight equally between the two halves: again, asymptotically, this gives a square root speedup. Combining these observations brings us to the following runtime and proba bility estimates for the Primal Hybrid algorithm with a MitM speedup. Lemma 3 (MitM Primal Hybrid Runtime). Assuming Heuristic 1, the runtime of Algorithm 2 with a MitM speedup is given by

TBKZ(β, d) + p | S| TNP(d)

Lemma 4 (MitM Primal Hybrid Success Probability). Algorithm 2 suc ceeds with probability

pNP · pmitm · Pr [sζ ∈ S] ,

where pNP and pmitm are determined by Lemmas 28 and 29 applied to displace ment (e, − ξsn− ζ ) and basis B′BKZ.

### 2.5 The Dual Hybrid Attack on LWE
We now review the code-based dual-hybrid attack of [23], which underlies the
best published concrete estimates for the Kyber/ML-KEM parameter sets [11,58].
We reproduce this algorithm as Algorithm 3. Unlike the Primal Hybrid, which
splits the columns of A into a guessing part (Aζ ) and a reduction part (An− ζ ),
this attack splits the columns of A, and so the secret coordinates, into three
parts:
1. Alat corresponding to slat: the part of A which we perform lattice reduction with respect to. These coordinates of the secret are fixed for all R trials
2. Aenu corresponding to senu: the part of the secret we guess is zero. This changes every trial
3. Afft corresponding to sfft: the part of the secret we exhaustively enumerate in order to verify our guess. This changes every trial We relate this algorithm to the generic hybrid guessing algorithm character isation of Algorithm 1 in the following way. For the sake of intuition, we omit decoding details. InitialWork() consists of finding vectors that make it easy to distinguish LWE samples with respect to Alat from random. Then suppose we guess a set of coordinates senu are zero. If we’re correct,

b = As + e = Alatslat + Afftsfft + e,

<!-- PDF page 10 -->

so that (Alat, b − Afftsfft) is a valid LWE sample. If our guess is wrong, this tuple will be uniformly distributed. Therefore, Check() determines whether there exists sfft such that (Alat, b − Afftsfft) is an LWE sample. This high level overview leaves out many details, which we will now expand on. In particular, we need to properly account for the false negative probability of this algorithm, and use a polar code to decode sfft into a dimension over which the exhaustive enumeration is feasible. We give a brief description of the various subroutines. Algorithm DualHybrid is fundamentally a distinguishing procedure (the event V ≥ T); we follow [23] and treat the subsequent recovery step SubLWE Solver as an additional conditional stage.

Algorithm 3 DualHybrid (Algorithm 3.1 of [23])
Input:
– Samples (A, b = As + e) ∈ Z m× n q × Z m q produced by an LWE(q, n, m, χs, χe)
oracle
– positive integers R, T, βbkz, βsieve, nenu, nfft, kfft, nlat, dlsc
– an [nfft, kfft]q linear code with generator matrix G ∈ Z nfft× kfft q
Output: the secret vector s ∈ Z n q (or ⊥ )
1: Choose Ilat ⊆ [n] such that | Ilat| = nlat.
2: Alat ← the columns of A indexed by Ilat.
3: S ← SetOfShortLatticeVectors(Alat, βsieve)
4: for i = 1 to R do
5: Choose a partition Ienu ∪ Ifft = [n] \ Ilat with | Ienu| = nenu and | Ifft| = nfft.
6: ▷ We now try to verify the guess senu = 0.
7: Aenu ← the columns of A indexed by Ienu.
8: Afft ← the columns of A indexed by Ifft.
9: ▷ Now use S to decode sfft into a lower dimension for exhaustive enumeration
10: L ← LWESamples(S , Afft, G, b)
11: V ← SolveLWEWithFFT(L ) ▷ Determine whether these are LWE samples
12: if V ≥ T then
13: senu ← 0 ▷ We have verified our guess
14: (sfft, slat) ← SubLWESolver [Afft Alat], b ▷ Solve for the rest of the key
15: return (senu, sfft, slat, Ienu, Ifft, Ilat)

16: return ⊥

Definition 7 (SetOfShortLatticeVectors). For Alat ∈ Z m× nlat q , write

S ← SetOfShortLatticeVectors(Alat, βsieve)

for the subroutine that constructs the q-ary dual lattice associated to Alat and runs a lattice sieve with block size βsieve to output a set S of short pairs (x, y) ∈Z m × Z nlat with y = A T latx (mod q).

Definition 8 (LWESamples). Write L ← LWESamples(S , Afft, G, b) for the following procedure.

<!-- PDF page 11 -->

For each (x, y) ∈ S , define ulsc ∈ Z kfft q as the output of decoding A T fftx to
the code generated by G, so that elsc := A T fftx − Gulsc is small. Then
LWESamples(S , Afft, G, b) outputs the list L := (ulsc,⟨ x, b⟩ ) : (x, y) ∈ S .
The utility of this step is that if senu = 0, it produces LWE samples with
respect to the secret G Tsfft. This is formalised by the following lemma.
Lemma 5. Let (A, b) be a collection of m LWE samples with secret and error
s and e, and let (ulsc,⟨ x, b⟩ ) : (x, y) ∈ S ← LWESamples(S , Afft, G, b)
as in Definition 8.
Then if senu = 0, each (ulsc,⟨ x, b⟩ ) is an LWE sample with respect to the
secret G Tsfft and error e′ := ⟨ x, e⟩ + ⟨ y, slat⟩ + ⟨ elsc, sfft⟩ .
Proof. Given in Appendix B.4.
It remains to explain how SolveLWEwithFFT assigns a “score" to these
LWE samples, which should be large if these are indeed LWE samples, and small
if they are uniform. This value can be computed more efficiently with an FFT:
we omit these details as they are extraneous to our purposes.
Definition 9 (SolveLWEWithFFT). Let L be a list of pairs (ulsc, b)
∈Z kfft q × Zq. For a candidate key z ∈ Z kfft q , define the score

F (lsc) 0 (z) := X (ulsc, b)∈L cos 2π q b − ⟨ ulsc, z⟩ .

Then SolveLWEWithFFT outputs the value V := maxz∈Z kfft q F (lsc) 0 (z). With these functions defined, we are able to give the runtime and success and failure probability of this algorithm.

#### 2.5.1 Runtime and Correctness

We refer to [23] for a detailed analysis of the runtime of each of these subroutines for different attack parameters. We will build on the estimator for this paper when estimating concrete costs. This lemma is a simplified analogue of their Theorem 4.1. Lemma 6 (DualHybrid Runtime). Assume that the cost of one call to SubLWESolver is negligible. Then the runtime of Algorithm 3 is

TSetOfShortLatticeVectors + R (TLWESamples + TSolveLWEWithFFT)

Of more concern to us is the correctness of this algorithm, as we will want to prove an analogous statement for our adaptation. In [23] correctness was captured by a single theorem and proof. We instead break the result into three modular results, so that we can reuse the analysis for our own algorithm. All of these results were proven as part of the central correctness proof of [23], we simply distribute them over three results for later convenience.

<!-- PDF page 12 -->

Lemma 7 (True Positive). We have that

Pr V ≥ T senu = 0 ≥ Pgood where Pgood := Pr F (lsc) 0 (G Tsfft) ≥ T senu = 0 .

Lemma 8 (False Positive). We have that

Pr V ≥ T senu ̸ = 0 ≤ q kfft · Pwrong where Pwrong := Pr F (lsc) 0 (z) ≥ T senu ̸ = 0 for z $ ← Z kfft q .

Now that we have both a lower bound on the probability of successfully identifying a correct guess, and an upper bound on incorrectly classifying an in correct guess, we can precisely capture the probability with which this algorithm succeeds. Lemma 9 (Dual Hybrid Correctness (Lemma 3.2 of [23])). Suppose A, b is sampled from an LWE(q, n, m, χs, χe) oracle, and assume SubLWESolver returns sfft, slat with probability 1 − µ whenever both senu = 0 and V ≥ T. Then the probability that Algorithm 3 succeeds in recovering the secret s is at least

η · Pgood · (1 − µ) − R · q kfft · Pwrong.

where, for z $ ← Z kfft q ,

Pgood := Pr F (lsc) 0 (G Tsfft) ≥ T senu = 0 , (1)

Pwrong := Pr F (lsc) 0 (z) ≥ T senu ̸ = 0 . (2)

and,

η := Pr[∃ i ∈ [R] : senu = 0] ,

where Ienu is the random choice made in a trial.
Proof. Given in Appendix B.4.
Remark 1. The bounds on Pgood and Pwrong concern the acceptance event V
≥T; the factor (1− µ) isolates the additional success probability of the subsequent
recovery step.
The probabilities Pgood and Pwrong are modelled and experimentally verified
in [23] in Assumptions 4.8 and 4.9. We will rely on these results for our own
Dual Hybrid attack.
In our restatement of this correctness result, we have left the final success
probability in terms of the hit probability η to make this dependence explicit,
as improving this factor using the module structure is the central consideration
of our work.

<!-- PDF page 13 -->

## 3 Primal Hybrid with Coefficient Isometries

In this section we will propose an adaptation of the primal hybrid attack for
Module Learning with Errors which exploits ring structure, which we will call
IsometricPrimalHybrid. The algorithm is given in Algorithm 4.
We start by summarising the Primal Hybrid for LWE given in Algorithm 2.
– we split the columns of A into An− ζ and Aζ . We perform BKZ reduction
on a lattice defined by An− ζ . This lattice (and its reduced basis) is fixed.
– For guesses sg of sζ , we try to decode the target t = b − Aζ sg to the lattice
defined by An− ζ .
Crucially, across all guesses the only fixed object is the lattice (and reduced
basis) defined by An− ζ ; the target changes with every guess. In plain LWE,
once the lattice is fixed this also fixes which ζ coordinates of the secret must be
guessed.
Recall the form of a single MLWE sample (a, b) over a ring R :

b =X

k i=1

aisi + e mod R q,

and observe that for any r ∈ R , we can form

rb =X

k i=1

ai(rsi) + re mod R q.

If re is also sampled from a “small" distribution, then we have that both (a, b) and (a, rb) are MLWE samples with respect to the same a vector: indeed, if re is from the same distribution as e, these are MLWE samples with the same distribution. This motivates the following definition. Definition 10 (Coefficient Isometry). Fix a ring R . Then r ∈ R is a coef ficient isometry if multiplication by r acts as a signed permutation on coefficient vectors, i.e. there exists a signed permutation matrix Πr such that

coeff(rx) = Πr coeff(x) for all x ∈ R .

This is quite a strict definition, and it may be sufficient that r preserves the Euclidean norm of the coefficient vector. However in our setting, we prefer this stronger property, as it preserves the distributions of interest to us. This is made concrete by the following definition and lemma.

Definition 11. A distribution D on R k is invariant under coefficient isometries if for every coefficient isometry r ∈ R ,

(rx)coeff

d = (x)coeff where x

$ ← D .

<!-- PDF page 14 -->

Lemma 10. The following distributions are invariant under coefficient isome
tries.
– the coefficients of x are i.i.d. from a centred symmetric distribution;
– (x)coeff is uniform over a fixed-Hamming-weight, centred symmetric set.
Looking ahead, this will be useful to us when we instantiate e with a Discrete
Gaussian sample and s with a fixed Hamming weight ternary sample; or, when
we sample both e, s coefficients from centred binomial distributions.
Returning to the Primal Hybrid, we see any non-trivial coefficient isome
try enables us to make guesses with respect to the same An− ζ , but different
coordinates of the secret. We formalise this using the following lemma.
Lemma 11. Let (A, b = As + e) have rows given by MLWE samples with

A ∈ R m× k q , s ∈ R k q, e ∈ R m q . Write n for the ring rank and set M := mn and

N := kn. Let Acoeff ∈ Z M× N q be the integer matrix satisfying

(Ax)coeff = Acoeff(x)coeff
Choose any index set J ⊆ [N] with |J | = ζ. Let Aζ ∈ Z M× ζ q be the sub-matrix
of Acoeff consisting of the columns indexed by J , and let AN− ζ ∈ Z M× (N− ζ) q
be the sub-matrix consisting of the remaining columns. For any x ∈ Z N q , write
xζ ∈ Z ζ q and xN− ζ ∈ Z N− ζ q for the corresponding sub-vectors (indices in J and
[N] \ J , respectively).
Define the augmented embedding lattice
Λq(AN− ζ ) := Λ qIM AN− ζ
0 ξIN− ζ ⊆ Z M+N− ζ.

Let r ∈ R q be a coefficient isometry, and assume the distributions of s and e are invariant under coefficient isometries. Write

bcoeff := (b)coeff, scoeff := (s)coeff, ecoeff := (e)coeff, for the flattened coefficient vectors. Then, in the quotient group Z M+N− ζ /Λq(AN− ζ ),

(rb)coeff − Aζ (rs)ζ 0

=

(re)coeff − ξ (rs)N− ζ (mod Λq(AN− ζ )).

Moreover,

(re)coeff − ξ (rs)N− ζ

d
=
ecoeff
− ξ sN− ζ .

Proof. Deferred to Appendix C.

<!-- PDF page 15 -->

| Algorithm | | 4 | IsometricPrimalHybrid | | | |
| --- | --- | --- | --- | --- | --- | --- |
| ⊥1: 2: 3: 4: 5: 6: 7: 8: 9: 10: | Input: A – m – guessing – a – guessing – MLWE – BKZ – Output: Compute M := Select columns Construct B′BKZ for each t ((re)coeff if return | ring R MLWE set of blocksize a Acoeff mn and ζ random form the BKZβ(BBKZ) ← (r, ((rb)coeff ← (r, sg,(rs)N− return ⊥ | of rank samples dimension coefficient S set error short ∈ N := columns AN− BKZ BBKZ sg) ∈ , ξ(rs)N− − (s, | n isometries ⊆ T variance β (s, e) N M× q Z kn. ζ Z ∈ basis = S do Aζ − 0 ζ , re) ζ e) ▷ | (A, b = As + e) A with ∈ ζ N N := kn where ≤ q T ⊆ R ζ q Z × σe and secret scaling factor As + e = b (mod q) with = (Ax)coeff Acoeff such that Acoeff of ; call the sub–matrix ζ) M× (N− q . qIM ζ d AN− d× , R 0 ξI ζ ∈ N− sg) mod q T ) (t) NPB′BKZ ← is a valid solution then s recover by undoing the signed | m k m× b , q q ∈ R R ξ = σe/σs or k x (x)coeff for all , where q ∈ R ζ M× Aζ . The remaining q Z ∈ d M + N ζ. ← − ▷ sg (rs)ζ guesses r permutation induced by |

Informally, this Lemma says that considering a new target obtained via mul tiplying by a coefficient isometry r does not change the distribution of the dis placement from the lattice defined by AN− ζ . Additionally, it means we are able to test guesses for (rs)ζ , rather than only sζ . We will see that for sparse secrets, this gives a significant advantage. We present IsometricPrimalHybrid in Algorithm 4, which adapts the Primal Hybrid to the MLWE setting using coefficient isometries.

### 3.1 Runtime and Correctness of IsometricPrimalHybrid
The runtime of Algorithm 4 follows identically to the plain case, except that the
search space S is now contained in T × Z ζ q.
Lemma 12 (Isometric Primal Hybrid, no MitM, Runtime). The run
time of Algorithm 4 is

TBKZ(β, d) + | S| TNP(d).

<!-- PDF page 16 -->

Algorithm 4 will succeed provided the guessing set contains a correct guess (r, sg) ∈ S with sg = (rs)ζ , and NP is able to correctly recover the displace ment (re)coeff, − ξ(rs)N− ζ using the basis B′BKZ. As r is a coefficient isome try, Lemma 11 implies the probability that the Nearest Plane succeeds is inde pendent of r. Lemma 13 (Isometric Primal Hybrid, no MitM, Success Probability). Algorithm 4 succeeds with probability

pNP · Pr[s ∈ S],

where pNP is determined by Lemma 28 applied to the basis B′BKZ and dis placement (e)coeff, − ξ(s)N− ζ , and Pr[s ∈ S] is shorthand for Pr∃ (r, sg) ∈ S such that (rs)ζ = sg .

Proof. The result follows by combining Lemma 11 and Lemma 28. ⊓⊔ We again combine these into a single security estimate following the bit security framework [55], considering the lowest possible value for runtime divided by success probability across all parametrisations of this algorithm.

### 3.2 MitM Speedup
We find it is still possible to achieve a Meet in the Middle square root speedup
for our algorithm, provided S = T × Splain exactly, and we can find an unbal
anced additive decomposition for Splain. This assumption is made concrete in
Heuristic 2. We give a sketch of exactly the MitM speedup we are proposing
in Algorithm 9 in Appendix C.1.
Heuristic 2 (Isometric S Decomposition) Let T be the set of coefficient
isometries used by Algorithm 4, and assume S = T × Splain for some Splain ⊆ Z ζ q.
There exist sets S1, S2 ⊆ Z ζ q such that

Splain = S1 + S2 := { s1 + s2 : s1 ∈ S1, s2 ∈ S2} , Moreover, we can balance S1, S2 such that |T | · | S1| ≈ | S2| , and hence |T | · | S1| + | S2| ≈ p | S| . Asymptotically, such a split can be justified similarly to Heuristic 1: for ex ample, we could extend Wunderer’s approach by splitting into two sets of weights hg 2 ±

log |T |4 , or extend May’s analysis by splitting the vector into two unequal lengths. Remark 2. This heuristic is perhaps more conservative than the previous heuris tic, Heuristic 1. We found that assuming the unbalanced decomposition instead of the balanced decomposition only made a few bits of difference to the final security estimates. We leave choosing between these two heuristics as a free parameter in our estimator.

<!-- PDF page 17 -->

Lemma 14 (MitM Isometric Primal Hybrid Runtime). Suppose S = T × Splain. Assuming Heuristic 2, the runtime of Algorithm 4 with a MitM speedup is given by

TBKZ(β, d) + p | S| TNP(d) Proof. Observe from Algorithm 9 that we make |T || S1| +| S2| calls to the Nearest Plane algorithm. The result follows from Heuristic 2, as this implies |T || S1| + | S2| =p | S| . ⊓⊔ Just as in the plain case, we can use the additive decomposition to “rebuild" the correct guess at runtime. However, the presence of isometries means we require some additional reasoning on why this is the case. Lemma 15 (MitM Isometric Primal Hybrid Success Probability). As sume S = T × Splain with Splain = S1 + S2. Then Algorithm 4 with a MitM speedup following Algorithm 9 succeeds with probability pNP · pmitm · Pr[s ∈ S],

where pNP and pmitm are determined by Lemmas 28 and 29 applied to the basis
B′BKZ and displacement (re)coeff, − ξ(rs)N− ζ , and Pr[s ∈ S] is shorthand for
Pr∃ (r, sg) ∈ S such that (rs)ζ = sg .
Proof. Given in Appendix C.1.
### 3.3 Power-of-Two Cyclotomics
Coefficient isometries will be different from ring to ring, and in many cases only
trivial isometries will exist. However, one ring of particular interest in cryptogra
phy is the power-of-two cyclotomic ring R = Z[X]/(X n + 1) for some power-of
two n. In these rings, multiplication by monomials X j has the effect of rotating
the coefficients negacyclically, so that all monomials are coefficient isometries.
This is shown formally with the following lemma.
Lemma 16. Let n be a power-of-two and let R q = Zq[X]/(X n + 1). For any
0 ≤ j < n, the monomial X j is a coefficient isometry.
Proof. Write f(X) =

P

n− 1 i=0 fiX i and identify f with its coefficient vector coeff(f) = (f0, . . . , fn− 1) T ∈ Z n q. In R q we have the relation X n ≡ − 1, so multiplying by

X gives

Xf(X) ≡ − fn− 1 +

n

X

− 2
i=0

fiX i+1 (mod X n + 1).

Then coeff(Xf) = Π coeff(f) where Π is the (negacyclic) rotation matrix (cf. [32])

Π

=

0 0 0 . . . − 1 1 0 0 . . . 0 0 1 0 . . . 0 ... ... ... ... ... 0 0 . . . 1 0 

.

<!-- PDF page 18 -->

This Π is a signed permutation matrix. Moreover, multiplication by X j corre sponds to multiplication by Π j, which is also a signed permutation as this set is closed under multiplication. Therefore each X j is a coefficient isometry by definition. ⊓⊔ Fixing R as the power-of-two cyclotomic ring and instantiating the set of coefficient isometries T with the negacyclic rotations { X j : 0 ≤ j < n} brings us to an algorithm RotPrimalHybrid, presented in Algorithm 5.

Algorithm 5 RotPrimalHybrid
Input: m MLWE samples (A, b) over R q = Zq[X]/(X n + 1) with n a power
of-two; guessing dimension ζ; plain guessing set Splain ⊆ Z ζ q ; other parameters as
in Algorithm 4.
Output: a short (s, e) with As + e ≡ b (mod q) or ⊥ .
1: T ← { X j : j = 0, 1, . . . , n − 1}2: S ← T × Splain
3: return IsometricPrimalHybrid(R , A, b, ζ, T , S, . . .)

The runtime and correctness of this algorithm follow from the runtime and correctness of IsometricPrimalHybrid (Lemmas 12 to 15). In the next sub section, we will look at concrete instantiation of the guessing set S in power-of two cyclotomics in more detail. Remark 3. We can understand this attack without reference to the ring structure by observing that matrices A that are formed of negacyclic blocks are defined by commuting with (powers of) the matrix Π (k) = Π ⊗ Ik, which is a signed permutation matrix. Using our algorithm for plain LWE would therefore neces sitate finding a signed permutation matrix of order n that commutes with an arbitrary given uniform matrix A. This requires A to be preserved up to sign under some simultaneous permutation of rows and columns, which is not true for arbitrary matrices.

#### 3.3.1 A Concrete Guessing Set for Fixed Hamming Weight Keys in Power-of-Two Cyclotomics

Although we have shown that coefficient isome tries enable a different guessing strategy in the Primal Hybrid algorithm applied to MLWE than is possible for plain LWE, we haven’t yet demonstrated this gives any advantage. We will demonstrate this in this subsection, by focusing on power-of-two cyclotomics, and the specific case of secrets with coefficients sampled from a bounded symmetric set such that the entire secret has fixed Hamming weight. We can define such secrets as having coefficient vector sam pled uniformly from the following set. Definition 12 (Fixed Hamming Weight Symmetric Set). For W ∈ 2Z, let W = {− W/2, ..., W/2} . Write W (h, n) := { v ∈ W n : hwt (v) = h}

<!-- PDF page 19 -->

i.e., the set of all length n vectors with entries from W and Hamming weight h.
Usually n will be clear from the context, and we will suppress it. For when
W = {− 1, 0, 1} , we recover the sparse ternary distribution, an extremely pop
ular choice for the secret key distribution in parametrisations of Homomorphic
Encryption.
For keys from this distribution, the standard guessing set 2 is to select a
maximum Hamming weight hg ≤ min(ζ, h), and guess all key segments of lower
weight.

Definition 13 (Guessing Set Splain(hg)). Let s

$ ← W (h). Then we define

Splain(hg) = { sg : sg ∈ W ζ and hwt (sg) ≤ hg}

It is straightforward to derive the size and hitting probability of this guessing

set.
Lemma 17 (Splain(hg) Size and Hitting Probability). Let s and Splain(hg)
be as in Definition 13. Then:

| Splain(hg)| =X

hg i=0

ζ i

W i, Pr[sζ ∈ Splain(hg)] = 1

n hX

hg i=0 n − ζ h − i

ζ
i
. (3)

To construct a guessing set for MLWE in power-of-two-cyclotomic rings, we build on this Splain(hg), considering all segments up to a certain weight, along with all possible rotations. Definition 14 (MLWE Guessing Set Srot(hg)). Let s ∈ (Z[X]/(X n + 1)) k have coefficient vector (s)coeff sampled uniformly from W (h, kn). We define

Srot(hg) = { X j : j ∈ [n]} × Splain(hg) = { (X j, sg) : j ∈ [n], sg ∈ W ζ and hwt (sg) ≤ hg} .

We find the hitting probability of Srot(hg) is well approximated by extending the heuristic introduced in [53]: namely, the weight on different rotations of the dropped coordinates is independent. Lemma 18 (Srot(hg) Size and Hitting Probability). Let Srot(hg) and Splain(hg) be as in Definitions 13 and 14 respectively, and write p(hg) = Pr[sζ ∈Splain(hg)], calculated as in Lemma 17. Then | Srot(hg)| = n | Splain(hg)| , Pr[s ∈ Srot(hg)] ≈ 1 − (1 − p(hg)) n.

Proof. Deferred to Appendix C.2. 2 see e.g. the Lattice Estimator here: L219 of prob.py, commit 1e28f66 or following [2]

<!-- PDF page 20 -->

To validate this heuristic, as well as determine whether these rotations give an advantage, we simulated the hitting probabilities of this set for a range of values of the ring rank n, module rank k, hamming weight h, and guessing dimension ζ, and for ternary keys. We plot the results in Fig. 1. From these experiments we make two observations. First, the independence heuristic predicts the simulated probability extremely well. Second, using Srot instead of Splain gives a significant advantage, having much greater hitting prob ability for the same number of guesses. For example, for the log n = 14 set (plot (d)), the guessing set that uses rotations needs 2 27 guesses to achieve a hit prob ability of 2− 7.9. The guessing set without rotations needs 2 78.7 guesses to achieve the same hit probability.

0 50 100

− 10 − 5

0

log | S|

l o g

S ]

Splain Srot, observed Srot, heuristic

(a)

log n = 8, h = 32, log ζ = 8, k = 4

0 50 100

− 15 − 10 − 5

0

log | S| (bits)

l o g

Splain Srot, observed Srot, heuristic

(b)

log n = 9, h = 64, log ζ = 8, k = 3

0 50 100

− 10 − 5

0

log | S|

l o g

S ]

Splain Srot, observed Srot, heuristic

(c)

log n = 10, h = 64, log ζ = 8, k = 2

0 100 200

− 20 − 10

0

log | S|

l o g

S ]

Splain Srot, observed Srot, heuristic

(d)

log n = 14, h = 64, log ζ = 12, k = 1

Fig. 1: Size of guessing set vs. hit probability for LWE compared to MLWE for a range of parameters. Coordinates are given by (log | S(hg)| , log Pr[s ∈ S(hg)])

(bits) for increasing values of hg. x-axis truncated for readability. 10000 trials.

Remark 4. Asymptotically, the independence heuristic implies our attack gives close to an O(n) advantage due to the ring structure for the Primal Hybrid.

<!-- PDF page 21 -->

In more detail, assuming the cost of the initial lattice reduction dominates the guessing phase, using rotations increases the hitting probability from p to 1− (1−p) n = O(np). In practice, the best attack will involve more carefully balancing the initial reduction and number of guesses, so we may not always be able to achieve exactly a gap of log n bits between LWE and MLWE in power-of-two cyclotomics. Remark 5. If we modelled each of the key segments X js ζ as fully independent, rather than just Hamming weight independent, we recover the guess one out of many keys problem, introduced in [41]. There, the authors find that for some secret distributions, this problem is easier than guessing a single key by a factor exponential in the length of the key to be guessed, in our case ζ. We leave exploring this connection to the enumeration literature to future work. The key conclusion from this section is that introducing isometries changes only the combinatorics of “hitting" a good guess (via Pr[s ∈ S]) while leaving the reduction and decoding part of the Primal Hybrid analysis unchanged. In the next section we adapt the same ideas to the Dual Hybrid, find we can strengthen the attack by changing the corresponding hit event.

## 4 Dual Hybrid with Coefficient Isometries

We now adapt the dual hybrid attack of [23] to MLWE by exploiting coefficient isometries. We call the resulting algorithm IsometricDualHybrid, and it is given in Algorithm 6. We briefly recall the Dual Hybrid for LWE given in Algorithm 3. – we fix a submatrix of A, Alat. We find short vectors in a lattice defined by Alat. This lattice is now fixed. – We make guesses at zero coordinates of the secret, and verify this guess is correct based on whether the (b′, A′) corresponding to this guess is an LWE sample. Across trials, the only fixed object is the set of short vectors computed from Alat. The set of LWE samples we verify changes with every guess. In plain LWE, once the column indices Ilat are fixed, we must guess at secret coordinates among the remaining [n]\ Ilat coordinates. We will see that coefficient isometries allow us to relax this restriction in the MLWE setting. We start by showing that invoking LWESamples on target (rb)coeff gen erates LWE samples of the same form as in the base attack. This is the dual hybrid analogue of Lemma 11, enabling us to argue that the underlying success and failure probabilities do not change when we introduce isometries. Lemma 19. Let (A, b) be a collection of m MLWE samples with secret and error s and e, and let

(ulsc,⟨ x,(rb)coeff⟩ ) : (x, y) ∈ S ← LWESamples(S , Afft, G,(rb)coeff)

<!-- PDF page 22 -->

as in Definition 8. Further suppose that both error and secret distribution are
invariant under coefficient isometries.
Then, if (rs)enu = 0, each (ulsc,⟨ x,(rb)coeff⟩ ) is an LWE sample with secret
G T(rs)fft and error

e′(r, x, y) := ⟨ x,(re)coeff⟩ + ⟨ y,(rs)lat⟩ + ⟨ elsc,(rs)fft⟩ ,

Moreover, writing e′(r) = e′(r, x, y) (x,y)∈ S for the induced error vector,

d = (G T(s)fft, e′,(s)enu).

(G T(rs)fft, e′(r),(rs)enu)

where e′ is the induced error vector corresponding to the identity isometry.
Proof. Given in Appendix D.

| Algorithm | | IsometricDualHybrid 6 | |
| --- | --- | --- | --- |
| 1: 2: 3: 4: 5: 6: 7: 8: 9: 10: 11: 12: 13: 14: 15: 16: | Input: A – m – a set – positive – an – Output: Construct where Choose Alat ← SetOfShortLatticeVectors(Alat, S ← i = for Choose Choose Aenu Afft L ← target V ← V if rb = A(rs) return | n ring of rank R (A, b = As + e) A MLWE samples with q of coefficient isometries T ⊆ R R, T, βbkz, βsieve, nenu, nfft, integers [nfft, kfft]q linear code with generator matrix k s the secret (or ) q ∈ R ⊥ N M× Acoeff (Ax)coeff such that q Z ∈ M := mn N := and kn. [N] = Ilat such that nlat. ⊆ | Ilat| Acoeff the columns of indexed by Ilat. βsieve) 1 R to do $ r . ← T = [N] Ienu Ifft Ilat a partition with ∪ \ ▷ We now Acoeff the columns of indexed by Ienu. ← Acoeff the columns of indexed by Ifft. ← , Afft, ) ▷ G,(rb)coeff LWESamples(S ) SolveLWEWithFFT(L T then ≥ ((rs)fft,(rs)lat) SubLWESolver [Afft ← + re s ▷ s recover by undoing the return ⊥ | k m m× b , q q ∈ R ∈ R kfft, nlat, dlsc kfft nfft× G q Z ∈ k = x Acoeff (x)coeff for all , q ∈ R = = nenu and nfft. | Ienu| | Ifft| = (rs)enu try to verify the guess 0. build samples using the permuted Alat], ▷ (rb)coeff solve w.r.t. r signed permutation induced by |

We can now analyse runtime and correctness exactly as in the non-isometric case.

<!-- PDF page 23 -->

#### 4.0.1 Runtime and Correctness of IsometricDualHybrid

We refer to [23] for a detailed analysis of the runtime of each subroutine. The isometric variant differs only in that each trial samples r ∈ T and replaces the target by (rb)coeff. we assume the cost of applying r (a signed permutation on coefficients) is negligible. Lemma 20 (IsometricDualHybrid Runtime). Assume that both the cost of one call to SubLWESolver and of applying any coefficient isometry is negligible. Then the runtime of Algorithm 6 is

TSetOfShortLatticeVectors + R (TLWESamples + TSolveLWEWithFFT).

Correctness follows the same argument as before, with the event senu = 0 replaced by (rs)enu = 0. For invariant secret and error distributions, Lemma 19 implies that the LWE instance seen by SolveLWEWithFFT in a trial has exactly the same distribution as in [23], and hence the same bounds apply. Lemma 21 (True Positive (Isometric)). Assume the secret and error dis tributions are invariant under coefficient isometries. Then for any coefficient isometry r used in Algorithm 6,

Pr V ≥ T (rs)enu = 0 ≥ Pgood,

where Pgood := Pr F (lsc)
0 (G Tsfft) ≥ T senu = 0 is as in Lemma 7.
Proof. Apply Lemma 7 to the list L (r) generated from (rb)coeff. Then use
Lemma 19 to replace (G T(rs)fft, e′(r),(rs)enu) by the identically distributed
(G Tsfft, e′, senu), giving Pgood. ⊓⊔
Lemma 22 (False Positive (Isometric)). Assume the secret and error dis
tributions are invariant under coefficient isometries. Then for any coefficient
isometry r used in Algorithm 6,

Pr V ≥ T (rs)enu ̸ = 0 ≤ q kfft · Pwrong, where Pwrong := Pr F (lsc) 0 (z) ≥ T senu ̸ = 0 is as in Lemma 8, and z $ ← Z kfft q .

Proof. The same argument applies: apply Lemma 8 to L (r), then substitute using Lemma 19. We combine these into one success probability in the same way as for the Dual Hybrid without isometries. Lemma 23 (Isometric Dual Hybrid Correctness). Suppose (A, b = As+ e) consists of m MLWE samples over R q, and assume the secret and error dis tributions are invariant under coefficient isometries. Assume SubLWESolver returns (rs)fft,(rs)lat with probability 1− µ whenever both (rs)enu = 0 and V ≥ T

<!-- PDF page 24 -->

occur in a trial. Then the probability that Algorithm 6 succeeds in recovering s is at least

η · Pgood · (1 − µ) − R · q kfft · Pwrong.

Here z $ ← Z kfft q and

Pgood := Pr F (lsc) 0 (G Tsfft) ≥ T senu = 0 , (4)

Pwrong := Pr F (lsc) 0 (z) ≥ T senu ̸ = 0 , (5)

and

η := Pr[∃ i ∈ [R] : (rs)enu = 0] ,

where (r, Ienu) are the random choices made in each trial. Proof. The proof is identical to Lemma 9, replacing Lemmas 7 and 8 with Lem mas 21 and 22. ⊓⊔ Comparing Lemma 23 and Lemma 9 we see that, for isometry-invariant se crets and errors, the analysis is unchanged except for the hit probability η. Recall that in each trial the algorithm chooses an enumeration set Ienu ⊂ [N]\ Ilat of size nenu. In the original attack,

η := Pr[∃ i ∈ [R] : senu = 0] ,

so success requires that the selected coefficients of s (restricted to [N]\ Ilat) are all zero in at least one trial. In our isometric variant,

η := Pr[∃ i ∈ [R] : (rs)enu = 0] ,

where r is sampled each trial. For nontrivial r, the condition (rs)enu = 0 cor responds s being zero on a signed-permuted set of coordinates, not necessarily contained in [N]\ Ilat. Across trials we therefore test a larger family of zero pat terns. This can strictly increase η.

### 4.1 Power-of-Two Cyclotomics

When the ring is a power-of-two cyclotomic ring, we instantiate T with the nega cyclic rotations { X j : 0 ≤ j < n} . This gives the algorithm RotDualHybrid, presented in Algorithm 7. This mirrors RotPrimalHybrid given by Algo rithm 5. Runtime and correctness of this algorithm follow directly from Lemmas 20 and 23. The remaining task is to estimate the hit probability η. We analyse this probability for secrets with coefficients i.i.d. from a symmetric centred distribu tion. This scenario matches the Kyber MLWE assumptions, to which we apply RotDualHybrid in Section 5. We start by giving the heuristic for the hit probability η from [23] (the non isometric case). Although [23] presents the following expression as an equality, it is most naturally justified under a conditional independence assumption between trials. We make this assumption explicit, since we will use an analogous heuristic in our extension to rotations.

<!-- PDF page 25 -->

| Algorithm 7 | RotDualHybrid |
| --- | --- |
| m MLWE Input: power-of-two; the Output: j X : j 1: T ← { 1}2: return IsometricDualHybrid(R | n (A, b = As + e) = Zq[X]/(X + 1) n q samples over with a R other parameters as in Algorithm 6. k s (or ). secret q ⊥ ∈ R = 0, 1, . . . , n − , A, b, , . . .) T |

Lemma 24 (Plain Hit Probability (Lemma 3.2 of [23])). Let s ∈ R

k have N coefficients, and let all coefficients be sampled independently and identically from a distribution D such that p0 := Pr[D → 0] > 0. Let N′ = N − nlat. Then the hit probability η := Pr[∃ i ∈ [R] : senu = 0] is well approximated by

R

t nenu

N′ t=0 1 −

N′ nenu!

p t 0(1 − p0) N′−

N′ t

.

η

t ≈

1 −X

Proof. Given in Appendix D.1. We extend the same model to rotations by assuming conditional indepen dence across trials given the number of zeros in the full secret. Lemma 25. Let R be the power-of-two cyclotomic ring of rank n, and suppose each trial samples r independently and uniformly from the set of isometries { X j :

j ∈ [n]} . Let s ∈ R

k have N = nk coefficients, and let all coefficients be sampled independently and identically from a distribution D such that

p0 := Pr[D → 0] > 0. Then the hit probability η := Pr[∃ i ∈ [R] : (rs)enu = 0] is well approximated by

R

t nenu

N t=0 1 −

N nenu!

p t 0(1 − p0) N− t.

N t

η ≈ 1 −X

Proof. Given in Appendix D.1. Remark 6. Empirically, assuming full independence across trials already pre dicts η reasonably well when using rotations. Under this assumption, η ≈ 1 −(1 − p nenu 0 ) R . This estimate slightly overestimates hit probability, but a more conservative estimate may be helpful for producing security estimates faster. For our results, we assume the more accurate Lemma 25. To validate both of these heuristics and establish that rotations give an ad vantage, we simulated the hitting probability with and without rotations for different values of the ring rank n, module rank k, centred binomial distribution with parameter α, dimensions nenu, nfft, nlat. We plot the results in Fig. 2. From these experiments we observe that the conditional independence heuris tics match the simulated probabilities extremely well, and that using rotations

<!-- PDF page 26 -->

gives a moderate advantage over not using rotations, in all cases having a larger hit probability η for the same number of trials R. The key conclusion is that coefficient isometries let the Dual Hybrid “reuse" the same short vectors while testing a larger set of zero patterns, all without changing the underlying correctness analysis.

0 200 400 600

0

0.2

0.4

0.6

0.8

1

R η

With Rotations Without Rotations

(a)

log n = 8, k = 2, α = 3 (Kyber512)

nenu = 5, nenu = 52

0 200 400 600

0

0.2

0.4

0.6

0.8

1

R η

With Rotations Without Rotations

(b)

log n = 8, k = 2, α = 3 (Kyber512)

nenu = 4, nenu = 48

0 200 400 600 800

0

0.2

0.4

0.6

0.8

1

R η

With Rotations Without Rotations

(c)

log n = 8, k = 3, α = 2 (Kyber768)

nenu = 6, nenu = 69

0 200 400 600 800

0

0.2

0.4

0.6

0.8

1

R η

With Rotations Without Rotations

(d)

log n = 8, k = 4, α = 2 (Kyber1024)

nenu = 6, nenu = 132

Fig. 2: Growth of hit probability η with number of guesses R with and without rotations. The approximations Lemmas 24 and 25 are marked with black crosses. We only include logarithmically many coordinates for readability. All parameters are either from our attack parameters or [23] attack parameters. 10000 trials.

## 5 Security Estimates

We now reevaluate the hardness of MLWE in power-of-two cyclotomic rings in light of our attacks. We focus on two parameter regimes that establish the impact of each of our algorithms: (i) sparse secret RLWE for the Primal Hybrid, and (ii) Kyber/ML-KEM MLWE assumptions [11,58] for the dual hybrid.

<!-- PDF page 27 -->

### 5.1 Primal Hybrid with Rotations

A clear trend in applications of RLWE has been the use of increasingly sparse secrets, e.g. ternary secrets of dimension n ∈ [2 14, . . . , 2 17] with Hamming weight as low as 32 [13,19,39]. In this regime, the Primal Hybrid attack is often the most performant attack [31,69], making it the natural setting to quantify the impact of our rotational symmetries. We evaluate RotPrimalHybrid (Algorithm 5) by extending the commu nity Lattice Estimator [7], commit 1e28f66. Concretely, we implement a new estimator routine that matches PrimalHybrid except for the guessing set size and hit probability computation, for which we use Lemma 18. we assume the Geometric Series Assumption and use the MATZOV SVP cost model [51] to match the HE standardisation process [18]. We additionally adopt a square-root meet-in-the-middle speedup at fixed success probability following Heuristic 2; for the corresponding results assuming Heuristic 1, see Appendix E.2. Our evaluation of RotPrimalHybrid addresses two questions: first, does ro tational symmetry in power-of-two cyclotomics create a measurable gap between sparse secret RLWE and the “equivalent” unstructured LWE instance with the same parameters; and second, what is the practical impact on sparse parameter sets proposed in the recent literature?

#### 5.1.1 Gap Between LWE and Sparse Secret RLWE

We consider a range of sparse parameters sets proposed for standardisation as 128-bit secure in [28,31]. We find that for these parameters, RotPrimalHybrid reveals a con crete gap between LWE and RLWE hardness in power-of-two cyclotomic rings in the sparse-secret regime, with the gap tending to widen as the secret becomes sparser. The takeaway is not that the ring structure makes the problem easy – the cost remains exponential across the range we study – but rather that the ring structure can induce significant security losses relative to the correspond ing LWE instance. In particular, for many proposed parameter sets, the RLWE estimates fall below the targeted security level even when the LWE estimates remain above it.

#### 5.1.2 Impact on Sparse RLWE in Practice

To assess practical impact,
we additionally surveyed recent Homomorphic Encryption publications from the
last calendar year at the top venues where applied Homomorphic Encryption
appears regularly, including Eurocrypt 2025, Crypto 2025, CCS 2025, and Asi
acrypt 2025, and report on all sparse parameters sets 3 We then recomputed their
concrete hardness under RotPrimalHybrid using our estimator. We report the
results in Table 8.
3 We remark that the task of determining parameters was in many cases surprisingly
difficult, especially for encapsulated keys. As security is the central consideration
to any HE implementation, we would encourage all authors to make explicit the
parameters for which they claim security, the security they claim, and the source for
the claimed security.

<!-- PDF page 28 -->

log n log q h σe LWE Security RLWE Security Gap

h = 64 (σe = 3.2)
11 25 64 3.2 150.1 142.6 7.5
12 52 64 3.2 143.5 134.8 8.7
13 99 64 3.2 146.2 136.7 9.5
14 219 64 3.2 141.3 130.5 10.8
15 431 64 3.2 145.2 133.0 12.2
16 930 64 3.2 142.5 129.7 12.8
17 2022 64 3.2 139.8 126.0 13.8
h = 128 (σe = 3.2)
11 42 128 3.2 145.9 137.2 8.7
12 82 128 3.2 142.5 132.9 9.6
13 165 128 3.2 139.9 129.2 10.7
14 337 128 3.2 138.0 126.4 11.6
15 700 128 3.2 135.5 123.0 12.5
16 1450 128 3.2 134.5 120.5 14.0
17 2900 128 3.2 136.4 121.6 14.8
h = 192 (σe = 3.19)
11 46 192 3.19 140.9 139.5 1.4
12 92 192 3.19 142.1 133.2 8.9
13 186 192 3.19 137.6 128.2 9.4
14 377 192 3.19 135.4 125.3 10.1
15 767 192 3.19 134.1 123.3 10.8

Table 1: Concrete hardness of LWE vs. RLWE using our attack, measured in bits. Parameters from Cheon et al. [28] (for h = 64, 128) and Curtis and Player [31] (for h = 192), which propose tables of sparse FHE parameters for practitioners. Bold indicates less than 128-bit security.

We find that for all but one paper ([56]) our attack drops at least one parame ter set below the claimed security level. This survey forces two conclusions: first, sparse secret parameter sets have become a mainstream choice in HE construc tions at top venues. Second, in this regime our attack has a significant impact: it produces substantial concrete security losses relative to the target security, with several instances dropping 15 bits. Taken together, Tables 1 and 8 establish that in the sparse secret setting, RLWE can be much easier than the corresponding sparse-secret LWE, even when both remain exponential-time. These results suggest that ring structure should be treated as a central consideration in the sparse regime.

### 5.2 Dual Hybrid with Rotations

We estimate the cost of RotDualHybrid (Alg. 7) under the three lattice reduction cost models used by [23,8]: Core-SVP (C0), classical circuit cost (CC),

<!-- PDF page 29 -->

and the classical query model (CN), all instantiated with the BDGL16 sieving oracle [10,15,5]. 4 Our implementation builds on the estimator of [23] 5 and match target suc cess/failure probabilities: namely we set Pgood ≈

1 2, so that the probability of correctly identifying MLWE samples is 1 2η: for our attacks, this is never less than 0.33. We find that the false positive probability ϵ is at most 2− 4.8, so that the overall advantage 1 2η − ϵ ≥ 0.33 for all parameters. Concrete costs are shown in Table 2, and the resulting MLWE–LWE gaps (relative to the LWE-only estimates of [23]) are shown in Table 3. Full attack parameters are given in Appendix E.1. Across all three parameter sets and all three cost models, rotations give a consistent concrete improvement over the LWE-only dual hybrid, yielding an MLWE hardness reduction of roughly 2–3 bits. This advantage arises solely from cyclotomic coefficient isometries increasing the probability of hitting a zero pattern event, while leaving the underlying distinguishing analysis unchanged. Consequently, transferring LWE-based estimates to MLWE in power-of-two cy clotomics without accounting for these symmetries is optimistic for the Ky ber/ML-KEM regime.

| Parameters Scheme q | MLWE Attack Complexity Security (Alg. 7) level n k α (NIST) C0 CC CN |
| --- | --- |
| 3329 Kyber512 | AES-128 256 2 3 118.8 137.1 132.2 (143 bits) |
| 3329 Kyber768 | AES-192 256 3 2 170.2 192.7 186.9 (207 bits) |
| 3329 Kyber1024 | AES-256 256 4 2 234.8 257.2 252.4 (272 bits) |

Table 2: MLWE parameters for Kyber/ML-KEM, NIST security targets, esti mated cost of our attack (bits) under various lattice reduction cost models.

## References

1. Albrecht, M., Chase, M., Chen, H., Ding, J., Goldwasser, S., Gorbunov, S., Halevi,
S., Hoffstein, J., Laine, K., Lauter, K., et al.: Homomorphic encryption stan
dard. In: Protecting privacy through homomorphic encryption, pp. 31–62. Springer
(2022)
4 CN corresponds to list_decoding_naive_classical in [5].
5 available at github.com/kevin-carrier/CodedDualAttack

<!-- PDF page 30 -->

| | | | | LWE | | | Hardness | | | | | | MLWE | Hardness | | | | | | | | | |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| | | | | | | | | | | | | | (Alg. | 7) | | | | Gap | | | | | |
| | Scheme | | | | | ([23]) | | | | | | | | | | | | | | | | | |
| | | | | C0 | | | CC | | CN | | | | C0 | CC CN | | C0 | | CC | | | CN | | |
| | Kyber512 | | 121.8 | | | 139.5 | | | 134.5 | | | 118.8 | | 137.1 132.2 | | 3.0 | | 2.4 | | | 2.3 | | |
| | Kyber768 | | 173.0 | | | 195.1 | | | 189.8 | | | 170.2 | | 192.7 186.9 | | 2.8 | | 2.4 | | | 2.9 | | |
| | Kyber1024 | | 239.0 | | | 259.7 | | | 254.6 | | | 234.8 | | 257.2 252.4 | | 4.2 | | 2.5 | | | 2.2 | | |
| Table | | 3: Gap | between | | | MLWE | | | and | LWE | | | hardness | Kyber for | parameters | | | | accord | | | | |

ing to our attack. All estimates presented in bits.

2. Albrecht, M.R.: On dual lattice attacks against small-secret LWE and param eter choices in HElib and SEAL. In: Coron, J.S., Nielsen, J.B. (eds.) EURO CRYPT 2017, Part II. LNCS, vol. 10211, pp. 103–129. Springer, Cham (Apr / May 2017). https://doi.org/10.1007/978-3-319-56614-6_4
3. Albrecht, M.R., Curtis, B.R., Deo, A., Davidson, A., Player, R., Postlethwaite, E.W., Virdia, F., Wunderer, T.: Estimate all the { LWE, NTRU} schemes! In: International Conference on Security and Cryptography for Networks. pp. 351– 367. Springer (2018)
4. Albrecht, M.R., Curtis, B.R., Wunderer, T.: Exploring trade-offs in batch bounded distance decoding. In: Paterson, K.G., Stebila, D. (eds.) SAC 2019. LNCS, vol. 11959, pp. 467–491. Springer, Cham (Aug 2019). https://doi.org/10.1007/ 978-3-030-38471-5_19
5. Albrecht, M.R., Gheorghiu, V., Postlethwaite, E.W., Schanck, J.M.: Estimat ing quantum speedups for lattice sieves. In: Moriai, S., Wang, H. (eds.) ASI ACRYPT 2020, Part II. LNCS, vol. 12492, pp. 583–613. Springer, Cham (Dec 2020). https://doi.org/10.1007/978-3-030-64834-3_20
6. Albrecht, M.R., Göpfert, F., Virdia, F., Wunderer, T.: Revisiting the expected cost of solving uSVP and applications to LWE. In: Takagi, T., Peyrin, T. (eds.) ASIACRYPT 2017, Part I. LNCS, vol. 10624, pp. 297–322. Springer, Cham (Dec 2017). https://doi.org/10.1007/978-3-319-70694-8_11
7. Albrecht, M.R., Player, R., Scott, S.: On the concrete hardness of learning with errors. Journal of Mathematical Cryptology 9(3), 169–203 (2015)
8. Albrecht, M.R., Shen, Y.: Quantum augmented dual attack. Cryptology ePrint Archive, Report 2022/656 (2022), https://eprint.iacr.org/2022/656
9. Alexandru, A., Kim, A., Polyakov, Y.: General functional bootstrapping using CKKS. In: Kalai, Y.T., Kamara, S.F. (eds.) CRYPTO 2025, Part III. LNCS, vol. 16002, pp. 304–337. Springer, Cham (Aug 2025). https://doi.org/10.1007/ 978-3-032-01881-6_10
10. Alkim, E., Ducas, L., Pöppelmann, T., Schwabe, P.: Post-quantum key ex change - A new hope. In: Holz, T., Savage, S. (eds.) USENIX Security 2016. pp. 327–343. USENIX Association (Aug 2016), https://www.usenix.org/conference/ usenixsecurity16/technical-sessions/presentation/alkim
11. Avanzi, R., Bos, J., Ducas, L., Kiltz, E., Lepoint, T., Lyubashevsky, V., Schanck, J.M., Schwabe, P., Seiler, G., Stehlé, D.: CRYSTALS-kyber: Algorithm specifica tions and supporting documentation (version 3.02). Round-3 submission to the

<!-- PDF page 31 -->

NIST Post-Quantum Cryptography Standardization Project (Aug 2021), https:
//pq-crystals.org/kyber/data/kyber-specification-round3-20210804.pdf
12. Babai, L.: On lovász’lattice reduction and the nearest lattice point problem. Com binatorica 6(1), 1–13 (1986)
13. Bae, Y., Cheon, J.H., Kim, J., Stehlé, D.: Bootstrapping bits with CKKS. In: Joye, M., Leander, G. (eds.) EUROCRYPT 2024, Part II. LNCS, vol. 14652, pp. 94–123. Springer, Cham (May 2024). https://doi.org/10.1007/978-3-031-58723-8_4
14. Bai, S., Galbraith, S.D.: Lattice decoding attacks on binary LWE. In: Susilo, W., Mu, Y. (eds.) ACISP 14. LNCS, vol. 8544, pp. 322–337. Springer, Cham (Jul 2014). https://doi.org/10.1007/978-3-319-08344-5_21
15. Becker, A., Ducas, L., Gama, N., Laarhoven, T.: New directions in nearest neighbor searching with applications to lattice sieving. In: Krauthgamer, R. (ed.) 27th SODA. pp. 10–24. ACM-SIAM (Jan 2016). https://doi.org/10.1137/ 1.9781611974331.ch2
16. Boneh, D., Kim, J.: Homomorphic encryption for large integers from nested residue number systems. In: Kalai, Y.T., Kamara, S.F. (eds.) CRYPTO 2025, Part III. LNCS, vol. 16002, pp. 338–370. Springer, Cham (Aug 2025). https://doi.org/10. 1007/978-3-032-01881-6_11
17. Bos, J.W., Costello, C., Ducas, L., Mironov, I., Naehrig, M., Nikolaenko, V., Raghu nathan, A., Stebila, D.: Frodo: Take off the ring! Practical, quantum-secure key exchange from LWE. In: Weippl, E.R., Katzenbeisser, S., Kruegel, C., Myers, A.C., Halevi, S. (eds.) ACM CCS 2016. pp. 1006–1018. ACM Press (Oct 2016). https://doi.org/10.1145/2976749.2978425
18. Bossuat, J.P., Cammarota, R., Chillotti, I., Curtis, B.R., Dai, W., Gong, H., Hales, E., Kim, D., Kumara, B., Lee, C., Lu, X., Maple, C., Pedrouzo-Ulloa, A., Player, R., Polyakov, Y., Lopez, L.A.R., Song, Y., Yhee, D.: Security guide lines for implementing homomorphic encryption. CiC 1(4), 26 (2024). https: //doi.org/10.62056/anxra69p1
19. Bossuat, J.P., Troncoso-Pastoriza, J.R., Hubaux, J.P.: Bootstrapping for approxi mate homomorphic encryption with negligible failure-probability by using sparse secret encapsulation. In: Ateniese, G., Venturi, D. (eds.) ACNS 2022. LNCS, vol. 13269, pp. 521–541. Springer, Cham (Jun 2022). https://doi.org/10.1007/ 978-3-031-09234-3_26
20. Brakerski, Z., Gentry, C., Vaikuntanathan, V.: (leveled) fully homomorphic encryp tion without bootstrapping. ACM Transactions on Computation Theory (TOCT) 6(3), 1–36 (2014)
21. Brakerski, Z., Langlois, A., Peikert, C., Regev, O., Stehlé, D.: Classical hardness of learning with errors. In: Boneh, D., Roughgarden, T., Feigenbaum, J. (eds.) 45th ACM STOC. pp. 575–584. ACM Press (Jun 2013). https://doi.org/10.1145/ 2488608.2488680
22. Buchmann, J.A., Göpfert, F., Player, R., Wunderer, T.: On the hardness of LWE with binary error: Revisiting the hybrid lattice-reduction and meet-in-the-middle attack. In: Pointcheval, D., Nitaj, A., Rachidi, T. (eds.) AFRICACRYPT 16. LNCS, vol. 9646, pp. 24–43. Springer, Cham (Apr 2016). https://doi.org/10.1007/ 978-3-319-31517-1_2
23. Carrier, K., Meyer-Hilfiger, C., Shen, Y., Tillich, J.P.: Assessing the impact of a variant of MATZOV’s dual attack on kyber. In: Kalai, Y.T., Kamara, S.F. (eds.) CRYPTO 2025, Part I. LNCS, vol. 16000, pp. 444–476. Springer, Cham (Aug 2025). https://doi.org/10.1007/978-3-032-01855-7_15

<!-- PDF page 32 -->

24. Castryck, W., Iliashenko, I., Vercauteren, F.: Provably weak instances of ring LWE revisited. In: Fischlin, M., Coron, J.S. (eds.) EUROCRYPT 2016, Part I. LNCS, vol. 9665, pp. 147–167. Springer, Berlin, Heidelberg (May 2016). https: //doi.org/10.1007/978-3-662-49890-3_6
25. Cheon, J.H., Choe, H., Kang, M., Kim, J., Kim, S., Mono, J., Noh, T.: Grafting: Decoupled scale factors and modulus in rns-ckks. Cryptology ePrint Archive (2024)
26. Cheon, J.H., Hanrot, G., Kim, J., Stehlé, D.: SHIP: A shallow and highly par allelizable CKKS bootstrapping algorithm. In: Fehr, S., Fouque, P.A. (eds.) EU ROCRYPT 2025, Part III. LNCS, vol. 15603, pp. 398–428. Springer, Cham (May 2025). https://doi.org/10.1007/978-3-031-91131-6_14
27. Cheon, J.H., Hhan, M., Hong, S., Son, Y.: A hybrid of dual and meet-in-the-middle attack on sparse and ternary secret lwe. IEEE Access 7, 89497–89506 (2019)
28. Cheon, J.H., Son, Y., Yhee, D.: Practical fhe parameters against lattice attacks. Journal of the Korean Mathematical Society 59(1), 35–51 (2022)
29. Choe, H., Kim, J., Stehlé, D., Suvanto, E.: Leveraging discrete ckks to bootstrap in high precision. Cryptology ePrint Archive (2025)
30. Coron, J.S., Seuré, T.: Paco: Bootstrapping for ckks via partial coefftoslot. Cryp tology ePrint Archive (2025)
31. Curtis, B.R., Player, R.: On the feasibility and impact of standardising sparse secret lwe parameter sets for homomorphic encryption. In: Proceedings of the 7th ACM Workshop on Encrypted Computing & Applied Homomorphic Cryptography. pp. 1–10 (2019)
32. Davis, P.J.: Circulant matrices, vol. 120. Wiley New York (1979)
33. Ducas, L., Engelberts, L., de Perthuis, P.: Predicting module-lattice reduction. In: International Conference on the Theory and Application of Cryptology and Information Security. pp. 133–166. Springer (2025)
34. Ducas, L., Kiltz, E., Lepoint, T., Lyubashevsky, V., Schwabe, P., Seiler, G., Stehlé, D.: Crystals-dilithium: A lattice-based digital signature scheme. IACR Transac tions on Cryptographic Hardware and Embedded Systems pp. 238–268 (2018)
35. Ducas, L., Nguyen, P.Q.: Learning a zonotope and more: Cryptanalysis of NTRUSign countermeasures. In: Wang, X., Sako, K. (eds.) ASIACRYPT 2012. LNCS, vol. 7658, pp. 433–450. Springer, Berlin, Heidelberg (Dec 2012). https: //doi.org/10.1007/978-3-642-34961-4_27
36. Eisenträger, K., Hallgren, S., Lauter, K.E.: Weak instances of PLWE. In: Joux, A., Youssef, A.M. (eds.) SAC 2014. LNCS, vol. 8781, pp. 183–194. Springer, Cham (Aug 2014). https://doi.org/10.1007/978-3-319-13051-4_11
37. Elias, Y., Lauter, K.E., Ozman, E., Stange, K.E.: Provably weak instances of ring LWE. In: Gennaro, R., Robshaw, M.J.B. (eds.) CRYPTO 2015, Part I. LNCS, vol. 9215, pp. 63–92. Springer, Berlin, Heidelberg (Aug 2015). https://doi.org/10. 1007/978-3-662-47989-6_4
38. Espitau, T., Joux, A., Kharchenko, N.: On a dual/hybrid approach to small secret LWE - A dual/enumeration technique for learning with errors and application to security estimates of FHE schemes. In: Bhargavan, K., Oswald, E., Prabhakaran, M. (eds.) INDOCRYPT 2020. LNCS, vol. 12578, pp. 440–462. Springer, Cham (Dec 2020). https://doi.org/10.1007/978-3-030-65277-7_20
39. Geelen, R., Vercauteren, F.: Fully homomorphic encryption for cyclotomic prime moduli. In: Fehr, S., Fouque, P.A. (eds.) EUROCRYPT 2025, Part III. LNCS, vol. 15603, pp. 366–397. Springer, Cham (May 2025). https://doi.org/10.1007/ 978-3-031-91131-6_13

<!-- PDF page 33 -->

40. Gentry, C.: Fully homomorphic encryption using ideal lattices. In: Mitzenmacher, M. (ed.) 41st ACM STOC. pp. 169–178. ACM Press (May / Jun 2009). https: //doi.org/10.1145/1536414.1536440
41. Glaser, T., May, A., Nowakowski, J.: Entropy suffices for guessing most keys. Cryp tology ePrint Archive, Report 2023/797 (2023), https://eprint.iacr.org/2023/797
42. Guimarães, A., Pereira, H.V.: Fast amortized bootstrapping with small keys and polynomial noise overhead. Cryptology ePrint Archive (2025)
43. Hirschhorn, P.S., Hoffstein, J., Howgrave-Graham, N., Whyte, W.: Choosing NTRUEncrypt parameters in light of combined lattice reduction and MITM ap proaches. In: Abdalla, M., Pointcheval, D., Fouque, P.A., Vergnaud, D. (eds.) ACNS 2009. LNCS, vol. 5536, pp. 437–455. Springer, Berlin, Heidelberg (Jun 2009). https://doi.org/10.1007/978-3-642-01957-9_27
44. Hwang, I., Min, S., Seo, J., Song, Y.: On the security and privacy of ckks-based homomorphic evaluation protocols. Cryptology ePrint Archive (2025)
45. Langlois, A., Stehlé, D.: Worst-case to average-case reductions for module lattices. DCC 75(3), 565–599 (2015). https://doi.org/10.1007/s10623-014-9938-4
46. Lee, C., Pellet-Mary, A., Stehlé, D., Wallet, A.: An LLL algorithm for mod ule lattices. In: Galbraith, S.D., Moriai, S. (eds.) ASIACRYPT 2019, Part II. LNCS, vol. 11922, pp. 59–90. Springer, Cham (Dec 2019). https://doi.org/10.1007/ 978-3-030-34621-8_3
47. Lindner, R., Peikert, C.: Better key sizes (and attacks) for LWE-based encryption. In: Kiayias, A. (ed.) CT-RSA 2011. LNCS, vol. 6558, pp. 319–339. Springer, Berlin, Heidelberg (Feb 2011). https://doi.org/10.1007/978-3-642-19074-2_21
48. Lyubashevsky, V.: Basic lattice cryptography: the concepts behind kyber (ml-kem) and dilithium (ml-dsa). Cryptology ePrint Archive (2024)
49. Lyubashevsky, V., Peikert, C., Regev, O.: On ideal lattices and learning with er rors over rings. In: Gilbert, H. (ed.) EUROCRYPT 2010. LNCS, vol. 6110, pp. 1–23. Springer, Berlin, Heidelberg (May / Jun 2010). https://doi.org/10.1007/ 978-3-642-13190-5_1
50. Lyubashevsky, V., Peikert, C., Regev, O.: A toolkit for ring-LWE cryptography. In: Johansson, T., Nguyen, P.Q. (eds.) EUROCRYPT 2013. LNCS, vol. 7881, pp. 35–54. Springer, Berlin, Heidelberg (May 2013). https://doi.org/10.1007/ 978-3-642-38348-9_3
51. MATZOV, I.: Report on the security of lwe: improved dual lattice attack, 2022. URL: https://zenodo. org/record/6412487
52. May, A.: How to meet ternary LWE keys. In: Malkin, T., Peikert, C. (eds.) CRYPTO 2021, Part II. LNCS, vol. 12826, pp. 701–731. Springer, Cham, Virtual Event (Aug 2021). https://doi.org/10.1007/978-3-030-84245-1_24
53. May, A., Silverman, J.H.: Dimension reduction methods for convolution modu lar lattices. In: International Cryptography and Lattices Conference. pp. 110–125. Springer (2001)
54. Micciancio, D., Mol, P.: Pseudorandom knapsacks and the sample complexity of LWE search-to-decision reductions. In: Rogaway, P. (ed.) CRYPTO 2011. LNCS, vol. 6841, pp. 465–484. Springer, Berlin, Heidelberg (Aug 2011). https://doi.org/ 10.1007/978-3-642-22792-9_26
55. Micciancio, D., Walter, M.: On the bit security of cryptographic primitives. In: Nielsen, J.B., Rijmen, V. (eds.) EUROCRYPT 2018, Part I. LNCS, vol. 10820, pp. 3–28. Springer, Cham (Apr / May 2018). https://doi.org/10.1007/ 978-3-319-78381-9_1

<!-- PDF page 34 -->

56. Moon, J., Yoo, D., Jiang, X., Kim, M.: Thor: Secure transformer inference with homomorphic encryption. In: Proceedings of the 2025 ACM SIGSAC Conference on Computer and Communications Security. pp. 3765–3779 (2025)
57. Mukherjee, T., Stephens-Davidowitz, N.: Lattice reduction for modules, or how to reduce ModuleSVP to ModuleSVP. In: Micciancio, D., Ristenpart, T. (eds.) CRYPTO 2020, Part II. LNCS, vol. 12171, pp. 213–242. Springer, Cham (Aug 2020). https://doi.org/10.1007/978-3-030-56880-1_8
58. National Institute of Standards and Technology: Module-lattice-based key encapsulation mechanism standard. Federal Information Processing Standards Publication 203, National Institute of Standards and Technology (Aug 2024). https://doi.org/10.6028/NIST.FIPS.203, https://nvlpubs.nist.gov/nistpubs/fips/ nist.fips.203.pdf
59. Nguyen, P.Q., Regev, O.: Learning a parallelepiped: Cryptanalysis of GGH and NTRU signatures. In: Vaudenay, S. (ed.) EUROCRYPT 2006. LNCS, vol. 4004, pp. 271–288. Springer, Berlin, Heidelberg (May / Jun 2006). https://doi.org/10. 1007/11761679_17
60. Nolte, N., Malhou, M., Wenger, E., Stevens, S., Li, C.Y., Charton, F., Lauter, K.E.: The cool and the cruel: Separating hard parts of LWE secrets. In: Vaudenay, S., Petit, C. (eds.) AFRICACRYPT 24. LNCS, vol. 14861, pp. 428–453. Springer, Cham (Jul 2024). https://doi.org/10.1007/978-3-031-64381-1_19
61. Park, J.H.: Ciphertext-ciphertext matrix multiplication: Fast for large matri ces. In: Fehr, S., Fouque, P.A. (eds.) EUROCRYPT 2025, Part VIII. LNCS, vol. 15608, pp. 153–180. Springer, Cham (May 2025). https://doi.org/10.1007/ 978-3-031-91101-9_6
62. Peikert, C.: How (not) to instantiate ring-LWE. In: Zikas, V., De Prisco, R. (eds.) SCN 16. LNCS, vol. 9841, pp. 411–430. Springer, Cham (Aug / Sep 2016). https: //doi.org/10.1007/978-3-319-44618-9_22
63. Postlethwaite, E.W., Virdia, F.: On the success probability of solving unique SVP via BKZ. In: Garay, J. (ed.) PKC 2021, Part I. LNCS, vol. 12710, pp. 68–98. Springer, Cham (May 2021). https://doi.org/10.1007/978-3-030-75245-3_4
64. Regev, O.: Lecture 1: Introduction (2004), https://cims.nyu.edu/~regev/teaching/ lattices_fall_2004/ln/introduction.pdf, lecture notes, Tel Aviv University, Fall 2004. Scribed by D. Sieradzki and V. Bronstein.
65. Regev, O.: On lattices, learning with errors, random linear codes, and cryptogra phy. In: Gabow, H.N., Fagin, R. (eds.) 37th ACM STOC. pp. 84–93. ACM Press (May 2005). https://doi.org/10.1145/1060590.1060603
66. Regev, O.: On lattices, learning with errors, random linear codes, and cryptogra phy. Journal of the ACM (JACM) 56(6), 1–40 (2009)
67. Rosca, M., Stehlé, D., Wallet, A.: On the ring-LWE and polynomial-LWE prob lems. In: Nielsen, J.B., Rijmen, V. (eds.) EUROCRYPT 2018, Part I. LNCS, vol. 10820, pp. 146–173. Springer, Cham (Apr / May 2018). https://doi.org/10.1007/ 978-3-319-78381-9_6
68. Schnorr, C.P.: A hierarchy of polynomial time lattice basis reduction algorithms. Theoretical computer science 53(2-3), 201–224 (1987)
69. Son, Y., Cheon, J.H.: Revisiting the hybrid attack on sparse secret lwe and appli cation to he parameters. In: Proceedings of the 7th ACM Workshop on Encrypted Computing & Applied Homomorphic Cryptography. pp. 11–20 (2019)
70. Wenger, E., Saxena, E., Malhou, M., Thieu, E., Lauter, K.E.: Benchmarking at tacks on learning with errors. In: Blanton, M., Enck, W., Nita-Rotaru, C. (eds.) 2025 IEEE Symposium on Security and Privacy. pp. 279–297. IEEE Computer Society Press (May 2025). https://doi.org/10.1109/SP61157.2025.00058

<!-- PDF page 35 -->

71. Wunderer, T.: A detailed analysis of the hybrid lattice-reduction and meet-in-the middle attack. Journal of Mathematical Cryptology 13(1), 1–26 (2019)

## A Detailed Prior Work Comparison

[60] RotPrimalHybrid RotDualHybrid Attack Family Cool & Cruel Primal Hybrid Dual Hybrid Lattice Reduction Dimension

Full Dimension Reduced Dimension Reduced Dimension Required Basis Shape Z-Shaped No special shape assumption

No special shape assumption

Guessing Dimension Determined by basis shape

Independently selected Independently selected

Role of Rotations Shift secret alignment to “cruel” basis vectors

Target different secret segments during hybrid guessing

Drop key coordinates from different index sets

Table 4: A comparison of our algorithms to prior work which uses rotations.

## B Additional Preliminaries

### B.1 Lattices

Definition 15 (Lattice [64]). A lattice is a discrete additive subgroup of R d for some d ≥ 1. Given n linearly independent vectors b1, ..., bn ∈ R d, the lattice generated by them is defined as their integer span. We refer to b1, ..., bn as a basis of the lattice. Equivalently, given a full column rank matrix B ∈ R d× n, the lattice generated by B is

Λ(B) = { Bx : x ∈ Z n } . We say that the rank of the lattice is n and the dimension of the lattice is d. There may be many bases that generate the same lattice. One invariant is the volume of a lattice. Definition 16 (Lattice Volume [64]). Given a lattice Λ generated by a basis B, define the volume of Λ as

vol(Λ) := √det B T B.

This quantity is independent of the choice of basis B. For any basis b1, ..., bn, we can compute the corresponding Gram–Schmidt or thogonalisation (GSO), which gives a set of orthogonal vectors which we denote b∗1, ..., b∗n. These are formally defined as follows.

<!-- PDF page 36 -->

Definition 17 (Gram–Schmidt Orthogonalisation (GSO) [64]). For any lattice basis b1, ..., bn, we define their Gram–Schmidt orthogonalisation as the sequence of vectors b∗1, ..., b∗n defined recursively by

b∗i = bi −X

i− 1
j=1

µi,jb∗j, where µi,j = bi· b∗j

b∗j· b∗j

.

We also define the fundamental parallelepiped of a basis. This region is not invariant under the choice of basis. Definition 18 (Fundamental parallelepiped). Let B = (b1, . . . , bn) ⊂ R d be a basis of a rank-n lattice Λ(B). The (half-open) fundamental parallelepiped of B is the set

P (B) := (X

n i=1 αibi αi ∈ −

1 2, 1 2)⊂ R d.

One technique for solving hard problems on lattices consists of finding a new basis for the same lattice, but with basis vectors which are short and near orthogonal. To this end, we will refer to the Block-Korkine-Zolotarev (BKZ) algorithm [68], which is parametrised by a blocksize β. For a comprehensive overview of the BKZ algorithm, we refer to [63]. For our purposes, we will only mention the Geometric Series Assumption (GSA) for the GSO profiles of a basis output by BKZ with blocksize β. Definition 19 (Geometric Series Assumption (Definition 3 of [6])). Let b1, ..., bd be the result of applying BKZ with blocksize β to a basis for a full-rank lattice with volume V , and let the corresponding GSO vectors be b∗1, ..., b∗d. Then the GSA gives that

∥ b∗i ∥ = δ

d d− 1(d+1− 2i) β V 1/d ≈ δ d+1− 2i β V 1/d,

with

δβ = (πβ) 1/β β 2πe !

1/(2(β− 1))

.

### B.2 Finding Close Vectors with Babai’s Nearest Plane Algorithm
We specify Babai’s Nearest Plane algorithm [12] in Algorithm 8.
We also use the following standard heuristic for the runtime of this algorithm
following [69,71,4].
Lemma 26 (NP Runtime [43]). The runtime of Babai’s nearest plane algo
rithm for a lattice in dimension d is well approximated by

TNP = d 2/2 1.06

<!-- PDF page 37 -->

Algorithm 8 Babai’s Nearest Plane Algorithm NPB(t)
Input A basis B = (b1, . . . , bn) ⊂ R d of a lattice Λ(B); the Gram–Schmidt or
thogonalisation b∗1, . . . , b∗n of B; a target t ∈ R d.
Output A vector e ∈ R d such that t − e ∈ Λ(B).
1: e ← t
2: for j = n down to 1 do

3: cj ← ⟨ e, b∗j ⟩ ⟨ b∗j, b∗j⟩4: e ← e − cj bj 5: return e

If we are searching for a particular shortest displacement e, as we are in (M)LWE attacks, then whether NPB(t) is successful depends on whether e can be found in the parallelepiped spanned by B∗, as captured by the following lemma. Lemma 27 ([71] Lemma 2.1.). Let B ⊂ Z d be a lattice basis, and let t be a target vector. Then NPB(t) returns the unique vector e ∈ P (B∗), that satisfies t − e ∈ Λ(B), where B∗ is the GSO orthogonalisation of B. If our basis consists of close to orthogonal vectors of similar length, this algo rithm can recover the displacement to the closest lattice point. However, if our basis is unreduced, the fundamental parallelepiped can be very long and narrow, and the algorithm does not recover the error to the closest vector, but instead to some other lattice point. We therefore assign to Babai’s Nearest Plane algorithm a success probability pNP which is approximated in the following lemma. Lemma 28 (NP Success Probability: Equation 4.1 of [71]). Let t be separated from the full rank lattice Λ(B) by a random vector e, and let the basis B have GSO vectors b∗1, ..., b∗d. Then if we let ri = ∥ b∗i ∥ 2 2∥ e∥

2, the probability NPB(t)

returns e is well approximated by:

pNP =Y

d

i=1 1 − 2 B( d− 1 2 , 1 2)Z

max(− ri,− 1)

− 1 (1 − x 2) d− 3 2 dx !

=Y

d i=1

I r 2 i ;

1
2

,

d − 1
2

,

where B(· , · ) denotes the Euler beta function, and I(x; a, b) is the cumulative distribution function of the Beta(a, b) distribution evaluated at x. Looking ahead to the Primal Hybrid algorithm, we will want to achieve a Meet in the Middle (MitM) square root speedup during the guessing phase. As we will see, this speedup requires NP to act as an additive homomorphism when we guess the two halves of the key segment correctly. The probability this succeeds is captured by the following lemma.

<!-- PDF page 38 -->

Lemma 29 (MitM Probability; Lemma 4.2 of [69]). Let t be separated from the lattice Λ(B) by a random Gaussian vector e with standard deviation σ, and suppose that NPB(t) → e. Let B have GSO vectors b∗1, ..., b∗n, and let ri = ∥ b∗i ∥√2σ. Then for any w ∈ R d,

pmitm = Pr [NPB(w) + NPB(t − w) = e] (6)

=Y

n i=1 erf(ri) − 1 − exp − r 2 i ri√π ! . (7)

where erf is the standard error function.
Having defined pNP, pmitm we can now analyse the success probability of the
Primal Hybrid algorithm.

### B.3 Primal Hybrid Details

Lemma (Restatement of Lemma 2). Algorithm 2 succeeds with probability

pNP · Pr [sζ ∈ S] , where pNP is determined by Lemma 28 applied to displacement (e, − ξsn− ζ ) and basis B′BKZ. Proof. Sketch. Observe that if sζ is in the guessing set S, for this guess we will have that

t = (b − Aζ sζ ) mod q 0

= (An− ζ sn− ζ + e) mod q 0

= BBKZ

w sn− ζ

+ e − ξsn− ζ

= B′BKZx + e − ξsn− ζ for some x ∈ Z d, as BBKZ and B′BKZ generate the same lattice. Therefore t is separated from the lattice spanned by B′BKZ by the random vector

e − ξsn− ζ : the result follows by Lemma 28. ⊓⊔ B.3.1 MitM Speedup Suppose we can write

S = S1 + S2 = { s1 + s2 : s1 ∈ S1, s2 ∈ S2}

Then, writing sζ ∈ S as sζ = s1 + s2, we can calculate

NP b − Aζ s1 0

, NP − Aζ s2
0
<!-- PDF page 39 -->

independently in an offline and online phase. If NP b − Aζ sζ
0
=
e
− ξsn− ζ ,
we will be able to reconstruct the missing part of the secret during the online
phase provided

NP b − Aζ s1 0

+ NP − Aζ s2 0 = NP b − Aζ sζ 0 = e − ξsn− ζ .

Conditioned on the second equality, the first equality is precisely the probability derived in Lemma 29.

### B.4 Dual Hybrid Details
Lemma (Restatement of Lemma 5). Let (A, b) be a collection of m LWE
samples with secret and error s and e, and let (ulsc,⟨ x, b⟩ ) : (x, y) ∈ S
←LWESamples(S , Afft, G, b) as in Definition 8.
Then if senu = 0, each (ulsc,⟨ x, b⟩ ) is an LWE sample with respect to the
secret G Tsfft and error e′ := ⟨ x, e⟩ + ⟨ y, slat⟩ + ⟨ elsc, sfft⟩ .
Proof. Using b = Afftsfft + Alatslat + e and bilinearity,

⟨ x, b⟩ = ⟨ A T fftx, sfft⟩ + ⟨ A T latx, slat⟩ + ⟨ x, e⟩ (mod q). Since A T latx = y (mod q), the middle term equals ⟨ y, slat⟩ . By definition of ulsc we have A T fftx = Gulsc + elsc, so

⟨ A T fftx, sfft⟩ = ⟨ ulsc, G Tsfft⟩ + ⟨ elsc, sfft⟩ .

The claim follows. ⊓⊔ Lemma (Restatement of Lemma 9). Suppose A, b is sampled from an LWE(q, n, m, χs, χe) oracle, and assume SubLWESolver returns sfft, slat with probability 1 − µ whenever both senu = 0 and V ≥ T. Then the probability that Algorithm 3 succeeds in recovering the secret s is at least

η · Pgood · (1 − µ) − R · q kfft · Pwrong.

where, for z $ ← Z kfft q ,

Pgood := Pr F (lsc) 0 (G Tsfft) ≥ T senu = 0 , (8)

Pwrong := Pr F (lsc) 0 (z) ≥ T senu ̸ = 0 . (9)

and,

η := Pr[∃ i ∈ [R] : senu = 0] ,

where Ienu is the random choice made in a trial.
Proof. Our algorithm will recover the correct secret whenever there exists a trial
for which the following three events happen:

<!-- PDF page 40 -->

– senu = 0,
– V ≥ T,
– SubLWESolver → sfft, slat.
By conditioning on each of these events in turn and applying Lemma 7, we have
that this happens with probability at least

ηPgood(1 − µ).

On the other hand, our algorithm will recover the wrong secret (or incorrectly distinguish) whenever there is a trial with:

– senu ̸ = 0,
– V ≥ T.
Using Lemma 8 and a union bound, we have the probability that this happens
with probability at most
R · q kfft
· Pwrong.

The claim follows. ⊓⊔

## C Primal Hybrid with Coefficient Isometries Details

Lemma (Restatement of Lemma 11). Let (A, b = As+e) have rows given by MLWE samples with A ∈ R m× k q , s ∈ R k q, e ∈ R

m q . Write n for the ring rank and set M := mn and N := kn. Let Acoeff ∈ Z M× N q be the integer matrix satisfying

(Ax)coeff = Acoeff(x)coeff Choose any index set J ⊆ [N] with |J | = ζ. Let Aζ ∈ Z M× ζ q be the sub-matrix of Acoeff consisting of the columns indexed by J , and let AN− ζ ∈ Z M× (N− ζ) q be the sub-matrix consisting of the remaining columns. For any x ∈ Z N q , write xζ ∈ Z ζ q and xN− ζ ∈ Z N− ζ q for the corresponding sub-vectors (indices in J and [N] \ J , respectively). Define the augmented embedding lattice Λq(AN− ζ ) := Λ qIM AN− ζ 0 ξIN− ζ ⊆ Z M+N− ζ. Let r ∈ R q be a coefficient isometry, and assume the distributions of s and e are invariant under coefficient isometries. Write

bcoeff := (b)coeff, scoeff := (s)coeff, ecoeff := (e)coeff, for the flattened coefficient vectors. Then, in the quotient group Z M+N− ζ /Λq(AN− ζ ), (rb)coeff − Aζ (rs)ζ 0 = (re)coeff − ξ (rs)N− ζ (mod Λq(AN− ζ )).

<!-- PDF page 41 -->

Moreover,

(re)coeff − ξ (rs)N− ζ

d
=
ecoeff
− ξ sN− ζ .

Proof. Since R q is commutative and r is multiplied componentwise, rb = r(As + e) = A(rs) + re ∈ R

m q

Applying the flattened coefficient embedding and the definition of Acoeff gives that

(rb)coeff = Acoeff(rs)coeff + (re)coeff (mod q) = AN− ζ (rs)N− ζ + Aζ (rs)ζ + (re)coeff (mod q).

Rearranging gives

(rb)coeff − Aζ (rs)ζ = AN− ζ (rs)N− ζ + (re)coeff (mod q).

Thus there exists w ∈ Z M such that (rb)coeff − Aζ (rs)ζ = qw + AN− ζ (rs)N− ζ + (re)coeff in Z M.

Therefore,

(rb)coeff − Aζ (rs)ζ 0 = qIM AN− ζ 0 ξIN− ζ

w (rs)N− ζ

+ (re)coeff − ξ(rs)N− ζ ,

so that the first claim follows by definition of Λq(AN− ζ ).
Finally, since r is a coefficient isometry, by Lemma 10 (applied component
wise) we have (re)coeff

d = ecoeff and (rs)coeff

d
= scoeff. Taking the sub-vector on
N − ζ coordinates preserves distributional equality for these distributions, so
that the claim follows. ⊓⊔
### C.1 MitM Speedup
Lemma (Restatement of Lemma 15). Assume S = T × Splain with Splain =
S1 +S2. Then Algorithm 4 with a MitM speedup following Algorithm 9 succeeds
with probability

pNP · pmitm · Pr[s ∈ S],

where pNP and pmitm are determined by Lemmas 28 and 29 applied to the basis B′BKZ and displacement (re)coeff, − ξ(rs)N− ζ , and Pr[s ∈ S] is shorthand for Pr∃ (r, sg) ∈ S such that (rs)ζ = sg .

<!-- PDF page 42 -->

Algorithm 9 Isometric Primal Hybrid with MitM (NP calls only)
Input: Aζ , B′BKZ, and a guessing set S = T × (S1 + S2), and b (MLWE target)
Output: Precomputed NP outputs (to be combined in the MitM step)
1: for each r ∈ T do
2: br ← (rb)coeff
3: for each s1 ∈ S1 do

4: t1(r, s1) ←

(br − Aζ s1) mod q 0 5: u1(r, s1) ← NPB′BKZ(t1(r, s1)) ▷ store u1(r, s1) in a table keyed by r, s1 6: for each s2 ∈ S2 do 7: t2(s2) ← (− Aζ s2) mod q 0 8: u2(s2) ← NPB′BKZ(t2(s2)) ▷ store u2(s2) in a table keyed by s2 9: return tables of u1 and u2

Proof. Suppose ∃ (r, sg) ∈ T × Splain with (rs)ζ = sg, and write sg = s1 + s2 with s1 ∈ S1, s2 ∈ S2. Then:

(e)coeff − ξ(s)N− ζ

d
=
(re)coeff
− ξ(rs)N− ζ
= NP (rb)coeff − Aζ sg
0
= NP (rb)coeff − Aζ s1
0

+ NP − Aζ s2
0

with probability pNP · pmitm. The result follows. ⊓⊔
### C.2 Hit Probability
Lemma (Restatement of Lemma 18). Let Srot(hg) and Splain(hg) be as
in Definitions 13 and 14 respectively, and write p(hg) = Pr[sζ ∈ Splain(hg)],
calculated as in Lemma 17. Then

| Srot(hg)| = n | Splain(hg)| , Pr[s ∈ Srot(hg)] ≈ 1 − (1 − p(hg)) n.

Proof. The size of the hitting set is immediate by definition. For the hitting probability,

Pr[s ∈ Srot(hg)] = Pr h ∃ j with hwt X js ζ ≤ hgi = 1 − Pr h ∀ j hwt X js ζ > hgi ≈ 1 − (1 − p(hg)) n .

This final equality assumes independence between the hamming weights of (X js)ζ for different j. ⊓⊔

<!-- PDF page 43 -->

## D Dual Hybrid with Coefficient Isometries Details

Lemma (Restatement of Lemma 19). Let (A, b) be a collection of m MLWE samples with secret and error s and e, and let

(ulsc,⟨ x,(rb)coeff⟩ ) : (x, y) ∈ S ← LWESamples(S , Afft, G,(rb)coeff)

as in Definition 8. Further suppose that both error and secret distribution are
invariant under coefficient isometries.
Then, if (rs)enu = 0, each (ulsc,⟨ x,(rb)coeff⟩ ) is an LWE sample with secret
G T(rs)fft and error

e′(r, x, y) := ⟨ x,(re)coeff⟩ + ⟨ y,(rs)lat⟩ + ⟨ elsc,(rs)fft⟩ ,

Moreover, writing e′(r) = e′(r, x, y) (x,y)∈ S for the induced error vector,

(G T(rs)fft, e′(r),(rs)enu)

d = (G T(s)fft, e′,(s)enu).

where e′ is the induced error vector corresponding to the identity isometry. Proof. Since rb = A(rs) + re,

(rb)coeff = Afft(rs)fft + Alat(rs)lat + Aenu(rs)enu + (re)coeff mod q.

If (rs)enu = 0, taking inner products with x gives that

⟨ x,(rb)coeff⟩ = ⟨ ulsc, G T(rs)fft⟩ + e′(r, x, y) (mod q), where we have used that A T latx = y mod q and A T fftx = Gulsc + elsc. This gives that (ulsc,⟨ x,(rb)coeff⟩ ) is an LWE sample with the claimed secret and error. For the distributional claim, since the secret and error distributions are invariant under coefficient isometries, by Definition 11, (rs)coeff

d = scoeff and (re)coeff

d = ecoeff. The claim follows. ⊓⊔

### D.1 Hit Probabilities

Lemma (Restatement of Lemma 24). Let s ∈ R

k have N coefficients, and let all coefficients be sampled independently and identically from a distribution D such that p0 := Pr[D → 0] > 0. Let N′ = N − nlat. Then the hit probability η := Pr[∃ i ∈ [R] : senu = 0] is well approximated by

η

≈

1 −X

N′ t=0 1 −

t nenu

N′ nenu!

R

N′ t

p t 0(1 − p0) N′−

t

.

<!-- PDF page 44 -->

Proof. Let sN′ be the vector of coefficients not selected by Ilat, and let T denote the number of zero coefficients in sN′ . Assume that the events

senu = 0 given T = t

are independent for all t ∈ [N′]. The result then follows by the law of total probability:

η = Pr[∃ i ∈ [R] : senu = 0] = 1 − Pr[∀ i ∈ [R] : senu ̸ = 0]

= 1 −X

N′ t=0 Pr[∀ i ∈ [R] : senu ̸ = 0 | T = t] Pr[T = t]

= 1 −X

N′ t=0

Pr[senu ̸ = 0 | T = t] R Pr[T = t] .

Finally, note that T ∼ Binomial(N′, p0), while

Pr[senu = 0 | T = t] =

t nenu N′ nenu

.

⊓⊔

Lemma (Restatement of Lemma 25). Let R be the power-of-two cyclotomic ring of rank n, and suppose each trial samples r independently and uniformly from the set of isometries { X j : j ∈ [n]} . Let s ∈ R

k have N = nk coeffi cients, and let all coefficients be sampled independently and identically from a distribution D such that

p0 := Pr[D → 0] > 0.

Then the hit probability η := Pr[∃ i ∈ [R] : (rs)enu = 0] is well approximated by

η ≈ 1 −X

N t=0 1 −

t nenu

N nenu!

R

N t

p t 0(1 − p0) N− t.

Proof. The proof follows the same conditioning argument as Lemma 24: let T denote the number of zero coefficients in s, and assume that the events

(rs)enu = 0 given T = t

<!-- PDF page 45 -->

are independent for all t ∈ [N]. The result then follows again by the law of total

probability, since η = Pr[∃ i ∈ [R] : (rs)enu = 0] = 1 − Pr[∀ i ∈ [R] : (rs)enu ̸ = 0]

N t=0 Pr[∀ i ∈ [R] : (rs)enu ̸ = 0 | T = t] Pr[T = t]

= 1 −X

N

Pr[(rs)enu ̸ = 0 | T = t] R Pr[T = t] .

= 1 −X

t=0

Finally, note that T ∼ Binomial(N, p0), while

Pr[(rs)enu = 0 | T = t] =

.

t nenu N nenu

⊓⊔

## E Additional Results

### E.1 Dual Hybrid

Scheme m βbkz βsieve nenu nfft kfft nlat dlat µlsc σlsc log2(N) log2(T)

Kyber512 488 378 378 1 38 8 473 3312.30 971.50 44.30 78.44 42.28 Kyber768 664 579 579 4 69 12 695 4663.97 1746.09 55.82 120.15 63.43 Kyber1024 909 800 800 17 101 16 906 5278.54 2325.98 41.63 166.01 86.66

C0

Kyber512 467 374 379 5 51 9 456 2913.10 1477.63 45.80 78.65 42.69 Kyber768 600 572 566 21 93 12 654 3849.41 2873.31 61.94 117.46 62.18 Kyber1024 791 802 784 25 133 17 866 4469.63 3432.21 55.22 162.69 85.02

CC

Kyber512 439 375 380 3 48 9 461 3046.83 1289.998 38.02 78.86 42.57 Kyber768 619 572 566 17 86 12 665 4100.70 2538.45 60.28 117.46 62.15 Kyber1024 774 808 789 13 133 18 878 4648.75 3222.31 53.70 163.73 85.54

CN

Table 5: Parameters used to obtain Tables 2 and 3 estimates.

| | Scheme | | | | log2(Pwrong) | | | | | | | log2(R) | | | | | log2(Tsample) | | | | | log2(N | · | Tdec) | | log2(TFFT) | | | | η | log2(ε) | |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| | Kyber512 | | | | | | | -105.57 | | | | | | 2.89 | | | | | 111.38 | | | | 115.84 | | | | 112.13 | | | 0.94 | -9.07 | |
| C0 | Kyber768 | | | | | | | -156.02 | | | | | | 6.67 | | | | | 170.07 | | | | 158.77 | | | | 159.52 | | | 0.86 | -8.93 | |
| | Kyber1024 | | | | | | | -233.08 | | | | | 25.06 | | | | | | 234.60 | | | | 204.64 | | | | 206.74 | | | 0.76 | -20.81 | |
| | Kyber512 | | | | | | | -158.79 | | | | | | 9.39 | | | | | 136.95 | | | | 116.05 | | | | 124.00 | | | 0.84 | -44.09 | |
| CC | Kyber768 | | | | | | | -177.02 | | | | | 30.72 | | | | | | 192.33 | | | | 156.08 | | | | 159.52 | | | 0.67 | -5.89 | |
| | Kyber1024 | | | | | | | -240.66 | | | | | 36.38 | | | | | | 256.90 | | | | 202.51 | | | | 218.53 | | | 0.66 | -5.37 | |
| | Kyber512 | | | | | | | -117.72 | | | | | | 6.06 | | | | | 131.88 | | | | 116.25 | | | | 124.00 | | | 0.86 | -6.35 | |
| CN | Kyber768 | | | | | | | -170.26 | | | | | 25.06 | | | | | | 186.58 | | | | 156.08 | | | | 159.52 | | | 0.73 | -4.80 | |
| | Kyber1024 | | | | | | | -239.22 | | | | | 19.40 | | | | | | 252.14 | | | | 203.55 | | | | 230.31 | | | 0.80 | -9.21 | |

Table 6: Intermediate parameters for Tables 2 and 3. Recall that Pgood ≈ 0.5. η and ε are as defined in Lemma 23.

<!-- PDF page 46 -->

### E.2 Primal Hybrid

We expand on the results given in Section 5.1, reporting on security estimates with a) no MitM speedup b) MitM speedup with probability Lemma 29 assum ing a guessing set decomposition under Heuristic 2 and c) MitM speedup with probability Lemma 29 assuming a guessing set decomposition under the existing Heuristic 1.

log n log q h σe

LWE, MitM, Heuristic 1 no MitM

MitM, Heuristic 1

MitM, Heuristic 2

h = 64 (σe = 3.2)
11 25 64 3.2 150.1 157.7 146.2 142.6
12 52 64 3.2 143.5 152.0 138.2 134.8
13 99 64 3.2 146.2 159.4 140.0 136.7
14 219 64 3.2 141.3 153.3 133.7 130.5
15 431 64 3.2 145.2 153.1 136.3 133.0
16 930 64 3.2 142.5 148.1 133.2 129.7
17 2022 64 3.2 139.8 142.2 129.5 126.0
h = 128 (σe = 3.2)
11 42 128 3.2 145.9 140.9 139.3 137.2
12 82 128 3.2 142.5 140.3 134.9 132.9
13 165 128 3.2 139.9 139.1 131.2 129.2
14 337 128 3.2 138.0 138.6 128.3 126.4
15 700 128 3.2 135.5 133.4 124.9 123.0
16 1450 128 3.2 134.5 130.2 122.3 120.5
17 2900 128 3.2 136.4 131.6 123.5 121.6
h = 192 (σe = 3.19)
11 46 192 3.19 140.9 139.5 143.2 141.6
12 92 192 3.19 142.1 135.1 134.8 133.2
13 186 192 3.19 137.6 132.2 129.5 128.2
14 377 192 3.19 135.4 131.0 126.6 125.3
15 767 192 3.19 134.1 129.5 124.7 123.3

Table 7: Concrete hardness of LWE vs. RLWE using our attack under different MitM assumptions, measured in bits. Parameters from Cheon et al. [28] (for h = 64, 128) and Curtis and Player [31] (for h = 192), which propose tables of sparse FHE parameters for practitioners. Bold indicates < 128-bit security.

We additionally report revised security estimates for sparse secret construc tions from the previous calendar year under all three of these MitM decomposi tions.

<!-- PDF page 47 -->

Venue Source Parameters RotPrimalHybrid Estimates (bits)

MitM, Heuristic 1

MitM, Heuristic 2

no MitM

log n log q h σe λ

13 55 31 3.2 128 130.7 117.5 113.0 14 100 31 3.2 128 140.7 121.9 116.9 15 105 31 3.2 128 177.5 144.6 138.9

[26]

EC’25

C’25

| [61] | 11 52 12 64 12 104 13 117 13 178 | 256 3.2 256 3.2 256 3.2 256 3.2 256 3.2 | 128 128.2 134.9 133.6 128 201.8 199.0 197.3 128 123.7 126.5 125.3 128 216.2 207.0 205.2 128 142.6 141.3 140.2 |
| --- | --- | --- | --- |
| [39] [9] | 14 420 14 120 15 767 16 1553 17 3104 | 256 3.2 32 3.2 192 3.19 192 3.19 192 3.19 | 128 121.3 119.8 119.0 128 132.7 117.7 113.2 128 129.5 124.7 123.3 128 128.7 123.8 122.6 128 130.2 124.9 123.8 |
| [16] [25] | 16 1518 16 104 15 679 15 780 16 1555 | 192 3.2 32 3.2 192 3.2 192 3.2 192 3.2 | 128 132.2 126.3 125.2 128 229.0 174.0 167.0 128 145.9 138.6 137.1 128 127.0 122.9 121.8 128 128.4 123.6 122.3 |
| [29] | 16 1533 16 118 | 192 3.2 30 3.2 | 128 130.3 125.3 123.8 128 212.9 161.7 156.3 |
| a [56] | 16 300 | 128 3.2 | 128 412.0 325.6 322.0 |
| [42] [44] | 11 64 11 64 12 64 13 64 12 64 13 64 14 404 | 49 35 2 47 38 2 43 30 2 43 25 2 40 32 2 41 26 2 256 ternary | 128 133.6 122.8 118.4 128 134.4 124.0 119.7 128 138.1 122.1 117.3 128 148.3 126.7 121.2 128 138.3 122.7 118.1 128 149.1 127.8 122.4 128 125.1 123.5 122.6 |

CCS’25

AC’25

[44] 14 404 256 ternary 128 125.1 123.5 122.6
[30]
15 934 64 3.2 100 92.2 86.6 85.1
16 1496 64 3.2 100 110.0 101.7 98.9

a We were unfortunately unable to confirm these parameters directly: our best estimate is the sparse parameters used by the DESILO library, used by this paper, which we find from here, parameter ID 8. Table 8: Impact of our attack on CKKS sparse parameter sets from the previous year of publications. We indicate in bold where (a variant of) our attack drops the parameters below the claimed security level λ.

<!-- PDF page 48 -->
