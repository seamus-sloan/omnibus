//  ComicPageView.swift
//  One comic page: a UIScrollView zoom wrapper around the page image.
//
//  UIKit rather than SwiftUI gestures because a comic page needs the whole
//  scroll-view contract at once — pinch to zoom about the pinch point,
//  pan while zoomed, double-tap to zoom, and *deference* while zoomed out
//  (an unzoomed page must not claim horizontal drags, or the TabView pager
//  behind it could never turn a page). UIScrollView gets all of that right
//  by construction; rebuilding it from MagnifyGesture does not.

import SwiftUI
import UIKit

/// Where a single tap landed, in page-turn terms.
enum ComicTapZone {
    case previous, toggle, next
}

/// The zoomable page image. Single taps report their zone (outer quarters
/// turn pages, the centre toggles chrome — the EPUB reader's gutter
/// contract); a double tap zooms in about the tap, or back out.
struct ComicPageView: UIViewRepresentable {
    let image: UIImage
    let onTap: (ComicTapZone) -> Void

    func makeUIView(context: Context) -> ComicZoomView {
        let view = ComicZoomView()
        view.onTap = onTap
        return view
    }

    func updateUIView(_ view: ComicZoomView, context: Context) {
        view.onTap = onTap
        view.display(image)
    }
}

/// The scroll-view host: owns fit-to-screen sizing, zoom centring, and the
/// tap recognizers.
final class ComicZoomView: UIScrollView, UIScrollViewDelegate {
    var onTap: ((ComicTapZone) -> Void)?

    private let imageView = UIImageView()
    /// Bounds the fit scale was computed against, so a rotation re-fits
    /// while ordinary layout passes leave the reader's zoom alone.
    private var fittedSize: CGSize = .zero

    override init(frame: CGRect) {
        super.init(frame: frame)
        delegate = self
        showsVerticalScrollIndicator = false
        showsHorizontalScrollIndicator = false
        contentInsetAdjustmentBehavior = .never
        // No bounce at 1x: a page that springs back on a horizontal drag
        // would swallow the exact gesture that should turn to the next one.
        bounces = false
        bouncesZoom = true
        backgroundColor = .black

        imageView.contentMode = .scaleAspectFill
        addSubview(imageView)

        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(didDoubleTap))
        doubleTap.numberOfTapsRequired = 2
        addGestureRecognizer(doubleTap)

        let singleTap = UITapGestureRecognizer(target: self, action: #selector(didSingleTap))
        singleTap.require(toFail: doubleTap)
        addGestureRecognizer(singleTap)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not used") }

    func display(_ image: UIImage) {
        guard imageView.image !== image else { return }
        imageView.image = image
        imageView.frame = CGRect(origin: .zero, size: image.size)
        contentSize = image.size
        fittedSize = .zero
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard let image = imageView.image, bounds.size != .zero else { return }
        if bounds.size != fittedSize {
            fittedSize = bounds.size
            fitToBounds(image)
        }
        centerContent()
    }

    /// Fit the page inside the viewport and make that the floor of the zoom
    /// range — zooming out past "whole page visible" serves nothing.
    private func fitToBounds(_ image: UIImage) {
        let fit = min(
            bounds.width / max(image.size.width, 1),
            bounds.height / max(image.size.height, 1)
        )
        minimumZoomScale = fit
        maximumZoomScale = max(fit * 4, 1)
        zoomScale = fit
    }

    /// Keep a page smaller than the viewport centred rather than pinned to
    /// the top-left, in both axes and at every zoom level.
    private func centerContent() {
        let x = max((bounds.width - contentSize.width) / 2, 0)
        let y = max((bounds.height - contentSize.height) / 2, 0)
        contentInset = UIEdgeInsets(top: y, left: x, bottom: y, right: x)
    }

    func viewForZooming(in scrollView: UIScrollView) -> UIView? { imageView }

    func scrollViewDidZoom(_ scrollView: UIScrollView) {
        centerContent()
    }

    @objc private func didSingleTap(_ gesture: UITapGestureRecognizer) {
        // Location relative to the viewport, not the (offset) content.
        let x = (gesture.location(in: self).x - contentOffset.x) / max(bounds.width, 1)
        if x < 0.25 {
            onTap?(.previous)
        } else if x > 0.75 {
            onTap?(.next)
        } else {
            onTap?(.toggle)
        }
    }

    @objc private func didDoubleTap(_ gesture: UITapGestureRecognizer) {
        if zoomScale > minimumZoomScale * 1.01 {
            setZoomScale(minimumZoomScale, animated: true)
            return
        }
        // Zoom in about the tap, so the panel under the finger is what grows.
        let target = zoomScale * 2.5
        let point = gesture.location(in: imageView)
        let size = CGSize(width: bounds.width / target, height: bounds.height / target)
        let origin = CGPoint(x: point.x - size.width / 2, y: point.y - size.height / 2)
        zoom(to: CGRect(origin: origin, size: size), animated: true)
    }
}
