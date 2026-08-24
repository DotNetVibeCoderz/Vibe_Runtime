using System;
using RustNet.UI;

namespace RustNet.Designer;

/// <summary>
/// Drag-to-move on the design canvas. Only elements whose parent is a
/// <c>canvas</c> are freely positionable (they carry absolute <c>x</c>/<c>y</c>);
/// layout-managed children (stack/grid/…) are placed by their container, so
/// dragging them is a no-op. Kept as pure model logic so it's unit-testable
/// without the WPF window.
/// </summary>
internal static class DragTool
{
    /// <summary>Can this element be moved by dragging (i.e. lives in a canvas)?</summary>
    public static bool CanMove(UiElement root, UiElement el)
    {
        if (el == root)
        {
            return false;
        }
        UiElement? parent = FindParent(root, el);
        return parent != null && parent.Kind == "canvas";
    }

    /// <summary>Nudge a canvas child by (dx, dy) logical pixels, clamped to
    /// non-negative coordinates. Returns true if it actually moved.</summary>
    public static bool MoveBy(UiElement root, UiElement el, int dx, int dy)
    {
        if (!CanMove(root, el) || (dx == 0 && dy == 0))
        {
            return false;
        }
        el.X = Math.Max(0, el.X + dx);
        el.Y = Math.Max(0, el.Y + dy);
        return true;
    }

    /// <summary>The parent of <paramref name="target"/> in the tree, or null
    /// if it is the root / not found.</summary>
    public static UiElement? FindParent(UiElement node, UiElement target)
    {
        for (int i = 0; i < node.Children.Count; i++)
        {
            UiElement c = node.Children[i];
            if (c == target)
            {
                return node;
            }
            UiElement? found = FindParent(c, target);
            if (found != null)
            {
                return found;
            }
        }
        return null;
    }
}
