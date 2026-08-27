# Flow catalog

Index of the flows an agent is handed, one at a time, by the runner. The
contract every agent reads first is [start.md](../start.md).

| Flow | Weight | Owner-only |
|---|---|---|
| [reading_a_book](reading_a_book.md) | 20% | no |
| [browsing_book_details](browsing_book_details.md) | 20% | no |
| [listening_to_audiobook](listening_to_audiobook.md) | 15% | no |
| [adding_book](adding_book.md) | 10% | — creates ownership |
| [sorting_the_library](sorting_the_library.md) | 8% | no |
| [browsing_authors](browsing_authors.md) | 8% | no |
| [browsing_series](browsing_series.md) | 8% | no |
| [wishlist](wishlist.md) | 5% | no |
| [creating_a_shelf](creating_a_shelf.md) | 4% | no — web can create but not fill |
| [updating_profile](updating_profile.md) | 2% | own account |
| [adding_highlight](adding_highlight.md) | 50% of a reading flow | no |
| [adding_bookmark](adding_bookmark.md) | 50% of a listening flow | no |
| [editing_metadata](editing_metadata.md) | 25% of a details flow | no |
| [adding_journal](adding_journal.md) | 25% of a details flow | no |
| [merging_books](merging_books.md) | 50% of an add-a-book flow | **yes, both books** |

The ten top-level flows sum to 100%; the rest run inside their parent. Weights
here are **suggested defaults** — the runner's configuration is authoritative,
and you never sample anything yourself. You are handed a flow; you execute it.

