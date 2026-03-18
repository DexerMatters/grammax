# Tree

As mentioned in [the previous chapter](./4-1-basic.md), The result of parsing is a concrete syntax tree (CST), which is a lossless representation of the source text. The CST is represented as a red-green tree, where the green nodes are immutable and shared between different versions of the tree, while the red nodes are mutable and represent the current version of the tree.