// Run: node --test scripts/test-citation-preview.mjs. Synthetic page layouts only: no library,
// no PDF, no browser. Each case is a layout that once read the wrong text out of a real paper.
import { test, after } from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readFile, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import ts from 'typescript'

const dir = await mkdtemp(join(tmpdir(), 'argus-citation-test-'))
after(() => rm(dir, { recursive: true, force: true }))
for (const name of ['citationPreview', 'highlightText', 'pageFurniture']) {
  const source = await readFile(new URL(`../src/utils/${name}.ts`, import.meta.url), 'utf8')
  const { outputText } = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2021, module: ts.ModuleKind.ESNext },
  })
  await writeFile(join(dir, `${name}.mjs`), outputText.replace(/from '(.\/[^']+)'/g, "from '$1.mjs'"))
}
const {
  findReferenceEntry, destinationFamily, destinationPoint, looksLikeReference, referencesHeading, labelAgrees,
} = await import(pathToFileURL(join(dir, 'citationPreview.mjs')))

// A span the way furniturePageFromTextContent reports one: top-left origin, top = baseline - 0.8 em.
// Widths are roughly Times at half an em per character unless given.
const span = (text, x, baseline, fontSize = 10, w = text.length * fontSize * 0.5) =>
  ({ text, x, y: baseline - 0.8 * fontSize, w, h: fontSize, fontSize })
const page = spans => ({ width: 612, height: 792, spans })
const lookup = (target, destinations, ...pages) =>
  findReferenceEntry({ target, destinations, pages: new Map(pages.map((p, i) => [target.page + i, p])) })
// hyperref's anchor: in the label box, ~25 pt left of an author-year entry, raised about a line.
const anchor = (name, textX, baseline, page = 0, offset = 25) => ({ page, x: textX - offset, y: baseline - 11.5, name })

test('natbib entry next to margin line numbers: the anchor sits on the numbers, the entry is the text', () => {
  const p = page([
    span('544', 73, 145, 8), span('545', 73, 156, 8), span('546', 73, 167, 8), span('547', 73, 178, 8),
    span('Zhengbao Jiang, Frank F Xu, Luyu Gao, Zhiqing Sun, Qian Liu, Jane Dwivedi-Yu, Yiming Yang,', 108, 145, 10, 396),
    span('Jamie Callan, and Graham Neubig. Active retrieval augmented generation. In Proceedings of the', 118, 156, 10, 386),
    span('2023 conference on empirical methods in natural language processing, pp. 7969–7992, 2023.', 118, 167, 10, 380),
    span('Lorenz Kuhn, Yarin Gal, and Sebastian Farquhar. Semantic uncertainty. arXiv, 2023.', 108, 182, 10, 396),
  ])
  const jiang = anchor('cite.jiang2023active', 108, 145)
  const kuhn = anchor('cite.kuhn2023semantic', 108, 182)
  const entry = lookup(jiang, [jiang, kuhn], p)
  assert.equal(entry.text, 'Zhengbao Jiang, Frank F Xu, Luyu Gao, Zhiqing Sun, Qian Liu, Jane Dwivedi-Yu, Yiming Yang, '
    + 'Jamie Callan, and Graham Neubig. Active retrieval augmented generation. In Proceedings of the '
    + '2023 conference on empirical methods in natural language processing, pp. 7969–7992, 2023.')
  assert.equal(entry.bounded, true)
})

test('an anchor raised a full line lands on the previous entry: the labelled line wins', () => {
  // CVPR: [60]'s anchor is exactly on the baseline of [59]'s last line.
  const p = page([
    span('[59] C. Szegedy, V. Vanhoucke, S. Ioffe, and J. Shlens. Rethinking the inception', 308.9, 509.4, 9, 236),
    span('architecture for computer vision. In CVPR, 2016.', 328.8, 520.4, 9, 216),
    span('[60] D. Tran, L. Bourdev, R. Fergus, L. Torresani, and M. Paluri. Learning', 308.9, 531.4, 9, 236),
    span('spatiotemporal features with 3D convolutional networks. In ICCV, 2015.', 328.8, 542.3, 9, 216),
    span('[61] D. Ulyanov, A. Vedaldi, and V. Lempitsky. Instance normalization, 2016.', 308.9, 553.3, 9, 236),
  ])
  const tran = { page: 0, x: 308.9, y: 520.4, name: 'cite.Tran2015' }
  const entry = lookup(tran, [tran], p)
  assert.match(entry.text, /^\[60\] D\. Tran/)
  assert.match(entry.text, /In ICCV, 2015\.$/)
})

test('a justified line written word by word is not taken for two columns', () => {
  // Every word is its own span, separated by ~1 em gaps (the space spans are blank).
  const words = (text, x, baseline) => {
    const out = []
    for (const w of text.split(' ')) { out.push(span(w, x, baseline)); x += w.length * 5 + 10 }
    return out
  }
  const p = page([
    ...words('Mostafa Dehghani, Stephan Gouws, Oriol Vinyals, Jakob Uszkoreit, and Lukasz Kaiser. Uni-', 71, 302),
    ...words('versal transformers. In International Conference on Learning Representations, 2019.', 80.5, 314),
    ...words('Xinyun Chen, Chen Liang, Adams Wei Yu, Dawn Song, and Denny Zhou. Compositional', 71, 334),
  ])
  const d = anchor('cite.dehghani2019universal', 71, 302)
  const next = anchor('cite.chen2020compositional', 71, 334)
  const entry = lookup(d, [d, next], p)
  assert.equal(entry.text, 'Mostafa Dehghani, Stephan Gouws, Oriol Vinyals, Jakob Uszkoreit, and Lukasz Kaiser. Uni-versal '
    + 'transformers. In International Conference on Learning Representations, 2019.')
})

// Lines of running text down a column: real pages are full, and a column is told by its edge
// being shared by many lines.
const filler = (x, from, to, step = 11, fontSize = 10) => {
  const out = []
  for (let b = from; b <= to; b += step) out.push(span('Running text of the other column, line by line', x, b, fontSize, 220))
  return out
}

test('two columns: the entry stays in its own column and continues at the top of the next one', () => {
  const p = page([
    ...filler(72, 450, 730, 13.5, 11),
    span('Dzmitry Bahdanau, Kyunghyun Cho, and Yoshua Ben-', 72, 752, 10, 220),
    span('gio. 2015. Neural machine translation by jointly', 82.9, 763, 10, 208),
    span('7564', 288, 786, 11),
    // Right column: the tail of Bahdanau, then the next entry.
    span('learning to align and translate. In Proceedings of', 318.2, 73.8, 10, 208),
    span('the International Conference on Learning Represen-', 318.2, 84.8, 10, 209),
    span('tations.', 318.2, 95.7, 10, 29),
    span('Joost Bastings, Marco Baroni, Jason Weston, Kyunghyun', 307.3, 115.9, 10, 220),
    span('Cho, and Douwe Kiela. 2018. Jump to better conclusions.', 318.2, 126.8, 10, 208),
    ...filler(307.3, 140, 780),
  ])
  const bahdanau = anchor('cite.Bahdanau14Attention', 72, 752, 0, 5)
  const bastings = anchor('cite.Bastings18Jump', 307.3, 115.9, 0, 5)
  const entry = lookup(bahdanau, [bahdanau, bastings], p)
  assert.equal(entry.text, 'Dzmitry Bahdanau, Kyunghyun Cho, and Yoshua Ben-gio. 2015. Neural machine translation by jointly '
    + 'learning to align and translate. In Proceedings of the International Conference on Learning Represen-tations.')
})

test('an entry continues on the next page, without that page\'s running head', () => {
  const p1 = page([
    span('Chaoqun He, Renjie Luo, and Maosong Sun. OlympiadBench:', 108, 700),
    span('743', 300, 770, 9),
  ])
  const p2 = page([
    span('Under review as a conference paper at ICLR 2027', 108, 40, 10),
    span('A challenging benchmark for promoting AGI with olympiad-level problems.', 118, 84),
    span('In Proceedings of ACL, pp. 3828–3850, 2024.', 118, 95),
    span('Zhengbao Jiang, Frank F Xu, and Graham Neubig. Active retrieval augmented', 108, 116),
    span('generation. In Proceedings of EMNLP, 2023.', 118, 127),
  ])
  const he = anchor('cite.he2024olympiadbench', 108, 700)
  const jiang = anchor('cite.jiang2023active', 108, 116, 1)
  const entry = lookup(he, [he, jiang], p1, p2)
  assert.equal(entry.text, 'Chaoqun He, Renjie Luo, and Maosong Sun. OlympiadBench: A challenging benchmark for promoting '
    + 'AGI with olympiad-level problems. In Proceedings of ACL, pp. 3828–3850, 2024.')
})

test('the next entry\'s anchor left at the bottom of the page: nothing is borrowed from the next page', () => {
  const p1 = page([
    span('Eleni E Litsa and Lydia E Kavraki. An end-to-end deep learning framework', 108, 708),
    span('for translating mass spectra. Communications Chemistry, 6(1):132, 2023.', 118, 719),
    span('10', 300, 770, 9),
  ])
  const p2 = page([
    span('Gang Liu, Jiaxin Xu, and Meng Jiang. Graph diffusion transformers. NeurIPS, 2024.', 108, 84),
    span('Yaron Lipman and Ricky TQ Chen. Flow matching for generative modeling. 2022.', 108, 100),
  ])
  const litsa = anchor('cite.litsa2023end', 108, 708)
  // Liu's anchor stayed on page 1, below the last line; Lipman's is on page 2.
  const liu = { page: 0, x: 83, y: 735, name: 'cite.liu2024graph' }
  const lipman = anchor('cite.lipman2022flow', 108, 100, 1)
  const entry = lookup(litsa, [litsa, liu, lipman], p1, p2)
  assert.equal(entry.text, 'Eleni E Litsa and Lydia E Kavraki. An end-to-end deep learning framework for translating mass '
    + 'spectra. Communications Chemistry, 6(1):132, 2023.')
})

test('an anchor without an x (FitH) in two columns: the column whose line starts an entry', () => {
  const p = page([
    // Left column: continuation lines (indented) near the point.
    span('Proceedings of the IEEE Conference on Computer Vision', 65.4, 112.9),
    span('3277, 2014.', 65.4, 124.9),
    span('Dodge, S. and Karam, L. A study of image quality.', 55.4, 145.9),
    // Right column: the entry, flush, continuation indented.
    span('Geirhos, R., Jacobsen, J.-H., Michaelis, C., and Zemel, R.', 307.4, 120.9),
    span('Shortcut learning in deep neural networks. arXiv, 2020.', 317.4, 132.9),
    span('Goodfellow, I. J., Shlens, J., and Szegedy, C. Explaining.', 307.4, 152.9),
    ...filler(55.4, 160, 700, 12), ...filler(307.4, 165, 700, 12),
  ])
  const geirhos = { page: 0, x: null, y: 108.9, name: 'cite.geirhos2020shortcut' }
  const next = { page: 0, x: null, y: 140.9, name: 'cite.goodfellow2014explaining' }
  const entry = lookup(geirhos, [geirhos, next], p)
  assert.equal(entry.text, 'Geirhos, R., Jacobsen, J.-H., Michaelis, C., and Zemel, R. Shortcut learning in deep neural '
    + 'networks. arXiv, 2020.')
})

test('numbered entries without anchors of their own end at the next label', () => {
  // MDPI: only [1] carries an anchor; "[CrossRef]" on an indented line is not a label.
  const p = page([
    span('1.', 36, 201, 9), span('Petrek, J.A.; Rosen, P.P. Lymphedema in a cohort of breast carcinoma survivors.', 57.5, 201, 9, 500),
    span('Cancer 2001, 92, 1368–1377.', 57.5, 212.7, 9),
    span('[CrossRef]', 57.5, 224.2, 9),
    span('2.', 36, 235.7, 9), span('Rockson, S.G.; Rivera, K.K. Estimating the population burden of lymphedema.', 57.5, 235.7, 9, 480),
  ])
  const first = { page: 0, x: 36, y: 189.7, name: 'cite.B1-biology' }
  const far = { page: 0, x: 36, y: 400, name: 'cite.B9-biology' }
  const entry = lookup(first, [first, far], p)
  assert.equal(entry.text, '1. Petrek, J.A.; Rosen, P.P. Lymphedema in a cohort of breast carcinoma survivors. '
    + 'Cancer 2001, 92, 1368–1377. [CrossRef]')
})

test('wide alpha labels: continuation lines far in are still the entry', () => {
  const p = page([
    span('[MKM+94]', 72, 300), span('Mitchell Marcus, Grace Kim, Mary Ann Marcinkiewicz, Robert MacIntyre, Ann Bies,', 135, 300, 10, 400),
    span('Mark Ferguson, Karen Katz, and Britta Schasberger. The Penn Treebank. 1994.', 135, 312, 10, 380),
    span('[MKAT18]', 72, 330), span('Sam McCandlish, Jared Kaplan, and Dario Amodei. An empirical model. 2018.', 135, 330, 10, 380),
  ])
  const marcus = { page: 0, x: 72, y: 288.5, name: 'cite.marcus1994penn' }
  const entry = lookup(marcus, [marcus, { page: 0, x: 72, y: 318.5, name: 'cite.mcc2018batchsize' }], p)
  assert.equal(entry.text, '[MKM+94] Mitchell Marcus, Grace Kim, Mary Ann Marcinkiewicz, Robert MacIntyre, Ann Bies, '
    + 'Mark Ferguson, Karen Katz, and Britta Schasberger. The Penn Treebank. 1994.')
})

test('wrapped words, URLs and separately drawn accents are put back together', () => {
  const p = page([
    span('Amir Taubenfeld and Gal Yona. Confidence improves self-consistency. In Findings of the Association for Com-', 108, 300, 10, 396),
    span('putational Linguistics: ACL 2025. Association for Computational Linguistics. URL https:', 118, 311, 10, 380),
    span('//aclanthology.org/2025.findings-acl.1030. Bernhard Sch', 118, 322, 10, 245),
    span('¨', 364, 322, 10, 4), span('olkopf, self-', 363, 322, 10, 60),
    span('consistency at 2.8 A', 118, 333, 10, 90), span('˚ ', 205, 331.5, 10, 4), span('ngstrom.', 209, 333, 10, 40),
  ])
  const t = anchor('cite.taubenfeld2025', 108, 300)
  const entry = lookup(t, [t], p)
  assert.equal(entry.text, 'Amir Taubenfeld and Gal Yona. Confidence improves self-consistency. In Findings of the Association '
    + 'for Computational Linguistics: ACL 2025. Association for Computational Linguistics. URL '
    + 'https://aclanthology.org/2025.findings-acl.1030. Bernhard Schölkopf, self-consistency at 2.8 Ångstrom.')
})

test('destination families: hyperref citations, publisher siblings, and what is never a reference', () => {
  assert.equal(destinationFamily('cite.li2025judging'), 'cite.')
  assert.equal(destinationFamily('bm_CR12'), 'bm_CR')
  assert.equal(destinationFamily('PAX@./papers/52.pax@15'), 'PAX@./papers/52.pax@')
  for (const name of ['figure.3', 'section.2', 'table.1', 'equation.4', 'page.12', 'Hfootnote.3', 'bm_Fig1', 'bm_Tab2', 'fig9005', 'appendix.A']) {
    assert.equal(destinationFamily(name), false, name)
  }
  assert.equal(destinationFamily(undefined), null)
  assert.equal(destinationFamily('intro'), null)
})

test('destination points: XYZ, FitH, and a Springer box as wide as the page', () => {
  const view = [0, 0, 612, 792]
  assert.deepEqual(destinationPoint([{ num: 1, gen: 0 }, { name: 'XYZ' }, 83, 657.8, null], view), { x: 83, y: 792 - 657.8 })
  assert.deepEqual(destinationPoint([{ num: 1, gen: 0 }, { name: 'FitH' }, 683], view), { x: null, y: 109 })
  // FitR [left bottom right top], written with top below bottom.
  assert.deepEqual(destinationPoint([{ num: 1, gen: 0 }, { name: 'FitR' }, 0, 307, 596, 298], [0, 0, 595.3, 782.4]),
    { x: null, y: 782.4 - 307 })
  assert.deepEqual(destinationPoint([{ num: 1, gen: 0 }, { name: 'FitR' }, 300, 100, 400, 120], view), { x: 300, y: 672 })
})

test('reference-like text, the reference heading, and link labels', () => {
  assert.equal(looksLikeReference('John Cocke. 1969. Programming languages and their compilers. New York University.'), true)
  assert.equal(looksLikeReference('132. Yu H, Zhao S, Shi J. STNN-DDI: a substructure-aware network. Brief Bioinform 2022;23:bbac209.'), true)
  assert.equal(looksLikeReference('63. McKinney, W. pandas: a foundational Python library. Python High Perf. Sci. Comput. 14, 1–9 (2011).'), true)
  assert.equal(looksLikeReference('4.1 Baseline Experiments We begin by reproducing the results of Smith et al. (2020).'), false)
  assert.equal(looksLikeReference('Figure 3: Accuracy by year, 2019 to 2023, for Smith et al.'), false)
  assert.equal(looksLikeReference('B Proofs of the main results in 2020'), false)

  assert.equal(referencesHeading(page([span('References', 72, 100, 12)])), true)
  assert.equal(referencesHeading(page([span('7', 72, 100, 12), span('References', 84, 100, 12)])), true)
  // IEEE small caps come apart into spans.
  assert.equal(referencesHeading(page([span('R', 280, 100, 10, 7), span('EFERENCES', 287, 100, 8, 50)])), true)
  assert.equal(referencesHeading(page([span('These references matter.', 72, 100)])), false)

  assert.equal(labelAgrees('45', '16. Wang, J. et al. Bromine-substituted fluorene. 2015.'), false)
  assert.equal(labelAgrees('[12]', '[12] A. Author. Title. 2020.'), true)
  assert.equal(labelAgrees('149]', '[182] M. J. Webber, R. Langer, Chem. Soc. Rev. 2017.'), false)
  assert.equal(labelAgrees('Li et al.', '[3] Li, X. Something. 2025.'), true)
})

test('a point in empty space or a rotated page gives no entry', () => {
  const p = page([span('Only one line far away.', 72, 100)])
  assert.equal(lookup({ page: 0, x: 72, y: 600, name: 'cite.x' }, [], p), null)
  assert.equal(lookup({ page: 0, x: 72, y: 88, name: 'cite.x' }, [], page([{ ...span('Rotated', 72, 100), angle: 90 }])), null)
  assert.equal(findReferenceEntry({ target: { page: 3, x: 1, y: 1 }, destinations: [], pages: new Map() }), null)
})
