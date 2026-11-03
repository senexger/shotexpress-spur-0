# Add the open framed coal wagon body (after coal_wagon_sketch.jpeg) and its
# drop-in coal lid to the EXISTING WagonBase document.
# Run: freecadcmd module_coal3.py
# Uses native expression-driven features; no Python proxy is needed on reopen.
# Its controls are appended to the shared Params sheet as coal3_*; the mounting
# dimensions (laenge, breite, radstand, modulplatte_dicke, modul_bohrung_d) are
# the base's own aliases, so the body follows frame changes live.
# Existing chassis geometry, modules and parameter values are preserved.

import hashlib
import os
import re
import xml.etree.ElementTree as ET
import zipfile

import FreeCAD as App
import MeshPart
import Part

OUT = os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals() else os.getcwd()
SOURCE = os.path.join(OUT, "WagonBase.FCStd")
FIN_COUNT = 7
RIVETS_PER_ROW = 18
VENT_ROWS = 6     # horizontal slot rows per long side, in the old nameplate field
VENT_COLUMNS = 3  # slots per row, separated by coal3_schlitz_steg
COAL_MARGIN = 0.4  # plain lid plate left around the coal
P = "Params."
C = "Params.coal3_"


def fingerprint(path):
    with open(path, "rb") as stream:
        return hashlib.sha256(stream.read()).hexdigest()


def box(name, length, width, height, x, y, z):
    obj = doc.addObject("Part::Box", "Coal3" + name)
    for prop, value in zip(("Length", "Width", "Height", "Placement.Base.x",
                            "Placement.Base.y", "Placement.Base.z"),
                           (length, width, height, x, y, z)):
        obj.setExpression(prop, str(value))
    return obj


def fuse(name, objects):
    obj = doc.addObject("Part::MultiFuse", "Coal3" + name)
    obj.Shapes = objects
    return obj


def cut(name, base, tool):
    obj = doc.addObject("Part::Cut", "Coal3" + name)
    obj.Base = base
    obj.Tool = tool
    return obj


def wedge(name, x0, x1, z0, z1, d0, d1, e0, e1, face="S", xt0=None, xt1=None):
    """Wall coordinates: x = along wall, z = height, d/e = inner/outer radius.

    A wedge's native Y is height. Its X/Z rectangle can translate and resize
    towards the top, so ribs and diagonal braces follow the hopper surface.
    """
    tangent, outward = {
        "S": (App.Vector(1, 0, 0), App.Vector(0, -1, 0)),
        "N": (App.Vector(-1, 0, 0), App.Vector(0, 1, 0)),
        "E": (App.Vector(0, 1, 0), App.Vector(1, 0, 0)),
        "W": (App.Vector(0, -1, 0), App.Vector(-1, 0, 0)),
    }[face]
    obj = doc.addObject("Part::Wedge", "Coal3" + name)
    obj.Placement.Rotation = App.Rotation(tangent, App.Vector(0, 0, 1), outward, "ZXY")
    values = dict(Xmin=x0, Xmax=x1, X2min=x0 if xt0 is None else xt0,
                  X2max=x1 if xt1 is None else xt1, Ymin=z0, Ymax=z1,
                  Zmin=d0, Zmax=e0, Z2min=d1, Z2max=e1)
    for prop, value in values.items():
        obj.setExpression(prop, str(value))
    return obj


def rising_slot(name, x0, x1, r0, r1, z0, h, rise_at_r0, face):
    """Through-wall slot, lower on the outside: radial r0 (inside) .. r1 (outside).

    Local axes: X = along the wall (reversed, to keep the frame right-handed),
    Y = outward, Z = up. The wedge's X/Z rectangle shifts down towards Ymax.
    """
    tangent, outward = {
        "S": (App.Vector(1, 0, 0), App.Vector(0, -1, 0)),
        "N": (App.Vector(-1, 0, 0), App.Vector(0, 1, 0)),
    }[face]
    obj = doc.addObject("Part::Wedge", "Coal3" + name)
    obj.Placement.Rotation = App.Rotation(-tangent, outward, App.Vector(0, 0, 1), "ZXY")
    zi = f"({z0})+({rise_at_r0})"
    values = dict(Xmin=f"-({x1})", Xmax=f"-({x0})", X2min=f"-({x1})", X2max=f"-({x0})",
                  Ymin=r0, Ymax=r1, Zmin=zi, Zmax=f"{zi}+({h})", Z2min=z0, Z2max=f"({z0})+({h})")
    for prop, value in values.items():
        obj.setExpression(prop, str(value))
    return obj


def ring(name,length, width, height, z, border):
    outer = box(name + "Outer", length, width, height,
                f"-({length})/2", f"-({width})/2", z)
    inner = box(name + "Inner", f"({length})-2*({border})", f"({width})-2*({border})",
                f"({height})+0.2", f"-({length})/2+({border})",
                f"-({width})/2+({border})", f"({z})-0.1")
    return cut(name, outer, inner)


def val(alias):
    value = getattr(sheet, alias)
    return float(value.Value if hasattr(value, "Value") else value)


def validate(label, check_coal=True):
    doc.recompute()
    errors = [(o.Name, o.State) for o in created
              if "Invalid" in o.State or "Error" in o.State]
    if errors:
        raise AssertionError(f"{label}: feature errors: {errors}")
    shape = final.Shape
    assert not shape.isNull() and shape.isValid(), f"{label}: invalid BRep"
    assert len(shape.Solids) == 1, f"{label}: disconnected parts"
    bb = shape.BoundBox
    assert abs(bb.ZMin) < 1e-6, f"{label}: underside must be flat at z=0"
    assert abs(bb.ZMax - val("coal3_oberkante")) < 1e-5, f"{label}: incorrect height"
    assert bb.XLength <= val("laenge") + 1e-5, f"{label}: length exceeds base"
    assert bb.YLength <= val("breite") + 1e-5, f"{label}: width exceeds base"
    # Each mounting hole must remain open, with a clear vertical driver/head
    # envelope above the floor, even after changing the body dimensions.
    for sx in (-1, 1):
        for sy in (-1, 1):
            x, y = sx * val("radstand") / 2, sy * 6
            bore = Part.makeCylinder(val("modul_bohrung_d") / 2 - 0.01,
                                     val("coal3_oberkante") + 1, App.Vector(x, y, -0.1))
            driver = Part.makeCylinder(2.4, val("coal3_oberkante"),
                                       App.Vector(x, y, val("modulplatte_dicke") + 0.01))
            assert shape.common(bore).Volume < 1e-6, f"{label}: blocked screw hole"
            assert shape.common(driver).Volume < 1e-6, f"{label}: blocked screw access"
    # The opening is clear down to the knee, apart from the narrow lid ledge.
    ledge = val("coal3_auflage")
    opening = Part.makeBox(val("coal3_innen_laenge") - 2 * ledge - 0.2,
                           val("coal3_innen_breite") - 2 * ledge - 0.2, val("coal3_oberkante"),
                           App.Vector(-val("coal3_innen_laenge") / 2 + ledge + 0.1,
                                      -val("coal3_innen_breite") / 2 + ledge + 0.1,
                                      val("coal3_knie") + 0.01))
    assert shape.common(opening).Volume < 1e-6, f"{label}: obstructed load opening"
    assert shape.common(base.Shape).Volume < 1e-6, f"{label}: chassis interference"
    # Vent slots: printable bars, clear of posts, rivets and lid ledge, open through the wall.
    pitch, slot, rise = (val("coal3_schlitz_abstand"), val("coal3_schlitz_hoehe"),
                         val("coal3_schlitz_anstieg"))
    length, gap, wall = val("coal3_schlitz_laenge"), val("coal3_schlitz_steg"), val("coal3_wand")
    assert pitch - slot >= 1 - 1e-9, f"{label}: vent bars thinner than 1 mm"
    assert gap >= 1 - 1e-9, f"{label}: vent column bars thinner than 1 mm"
    half_field = (VENT_COLUMNS * length + (VENT_COLUMNS - 1) * gap) / 2
    post_edge = 0.18 * val("coal3_kasten_laenge") - val("coal3_traeger_breite") / 2
    assert half_field + 0.8 <= post_edge, f"{label}: vents run into the inner posts"
    z_bottom = val("coal3_schlitz_unten")
    z_top = z_bottom + val("coal3_schlitz_feld")
    assert z_bottom > val("coal3_knie") + 2.5, f"{label}: vents reach the knee rivets"
    assert z_top < val("coal3_oben") - val("coal3_gurt_hoehe") - 2.5, f"{label}: vents reach the top rivets"
    assert z_top < val("coal3_deckel_unten") - ledge - 0.5, f"{label}: vents reach the lid ledge"
    r_out = val("coal3_kasten_breite") / 2
    for sy in (-1, 1):
        for j in range(VENT_ROWS):
            for c in range(VENT_COLUMNS):
                x = (length + gap) * (c - (VENT_COLUMNS - 1) / 2)
                for depth in (0.05, wall / 2, wall - 0.05):   # along the rising slot
                    z = z_bottom + j * pitch + slot / 2 + rise * depth / wall
                    for dx in (-length / 2 + 0.1, 0, length / 2 - 0.1):
                        point = App.Vector(x + dx, sy * (r_out - depth), z)
                        assert not shape.isInside(point, 1e-6, True), f"{label}: vent slot not open"
    # Lid: drops through the rim, rests on the ledge, clears the body.
    lid_shape = lid.Shape
    assert lid_shape.isValid() and shape.common(lid_shape).Volume < 1e-6, f"{label}: lid collides"
    assert val("coal3_deckel_laenge") < val("coal3_kasten_laenge") - 2 * val("coal3_rand_wand"), \
        f"{label}: lid does not pass the rim"
    for size in ("laenge", "breite"):
        overlap = val(f"coal3_deckel_{size}") / 2 - (val(f"coal3_innen_{size}") / 2 - ledge)
        assert overlap >= 0.8, f"{label}: lid rests on less than 0.8 mm of ledge"
    assert abs(lid_shape.BoundBox.ZMin - val("coal3_deckel_unten")) < 1e-6
    assert val("coal3_deckel_unten") - ledge > val("coal3_knie") + 1, f"{label}: lid below the knee"
    if check_coal:   # the coal mesh footprint is fixed when the script runs
        local = coal.Mesh.copy()
        local.Placement = App.Placement()
        cb = local.BoundBox
        z0 = coal.Placement.Base.z
        assert max(abs(cb.XMin), cb.XMax) <= val("coal3_deckel_laenge") / 2 - COAL_MARGIN + 1e-4 and \
            max(abs(cb.YMin), cb.YMax) <= val("coal3_deckel_breite") / 2 - COAL_MARGIN + 1e-4, \
            f"{label}: coal overhangs lid"
        top = z0 + cb.ZMax
        assert abs(top - (val("coal3_oberkante") + val("coal3_kohle_ueber"))) < 1e-3, \
            f"{label}: coal top {top:.3f} not at rim top"
        assert abs(z0 + coal_bottom - (val("coal3_deckel_oben") - 0.05)) < 1e-6
    print(f"{label}: valid single solid, {bb.XLength:.1f} x {bb.YLength:.1f} x "
          f"{bb.ZLength:.1f} mm; four accessible mounting holes, "
          f"{2 * VENT_ROWS * VENT_COLUMNS} open vents; lid at z={val('coal3_deckel_unten'):.2f}",
          flush=True)


source_hash = fingerprint(SOURCE)
with zipfile.ZipFile(SOURCE) as archive:
    root = ET.fromstring(archive.read("Document.xml"))
    params_xml = root.find("./ObjectData/Object[@name='Params']")
    original_cells = {c.get("address"): c.get("content", "") for c in params_xml.iter("Cell")}
    first_row = max(int(re.search(r"\d+", a).group()) for a in original_cells) + 2
doc = App.openDocument(SOURCE)
sheet = doc.getObject("Params")
base = doc.getObject("WagonBase")
if doc.getObject("ModuleCoal3") or any(o.Name.startswith("Coal3") for o in doc.Objects):
    raise RuntimeError("ModuleCoal3 already exists. Edit Params in WagonBase.FCStd; "
                       "do not reinstall over existing work.")
preserved = {o.Name: o.Shape.copy() for o in doc.Objects
             if not o.InList and o.isDerivedFrom("Part::Feature")}
existing_names = {obj.Name for obj in doc.Objects}
visible_before = {obj.Name: obj.Visibility for obj in doc.Objects}

# Reuse the existing coal surface (CoalLoad, generated by wagon_base.py).
# Its points are local; the bottom sits at coal_bottom and overlaps the plate by 0.05.
coal_src = doc.getObject("CoalLoad").Mesh.copy()
coal_src.Placement = App.Placement()
coal_bb = coal_src.BoundBox
coal_bottom = coal_bb.ZMin
coal_height = coal_bb.ZMax - coal_bottom - 0.05

categories = [
    ("Kohlemodul 3 — offener Wagenkasten nach Skizze (Maße in mm)", (0.22, 0.30, 0.38), [
        ("coal3_hoehe", 55, "Coal3: Kastenhöhe über dem Boden, ohne erhöhten oberen Rand; empfohlen 35–75"),
        ("coal3_wand", 1.8, "Coal3: Wandstärke horizontal gemessen; empfohlen 1.6–2.4"),
        ("coal3_einzug", 2, "Coal3: Abstand der oberen Wandaußenseite zur Grundplattenkante"),
        ("coal3_verjuengung", 2.5, "Coal3: zusätzlicher Einzug unten je Seite; empfohlen 1–3"),
        ("coal3_knick_anteil", 0.24, "Coal3: Anteil der Kastenhöhe mit geneigter Wand; empfohlen 0.20–0.35"),
        ("coal3_traeger_breite", 2.6, "Coal3: Breite der senkrechten Rippen und Eckpfosten"),
        ("coal3_relief", 1.4, "Coal3: Rippen-/Strebenhöhe außen; kleiner als coal3_einzug"),
        ("coal3_strebe_breite", 1.8, "Coal3: Breite der diagonalen Streben, entlang X gemessen"),
        ("coal3_gurt_hoehe", 2.4, "Coal3: Höhe der oberen und unteren umlaufenden Gurte"),
        ("coal3_rand_hoehe", 3.5, "Coal3: erhöhter offener Rand für spätere Kohleladung"),
        ("coal3_rand_wand", 1.6, "Coal3: Stärke des erhöhten Randes"),
        ("coal3_rippe_breite", 1.2, "Coal3: Stärke der sieben Stirnwandrippen"),
        ("coal3_niet_d", 1.2, "Coal3: Nietdurchmesser; kleine halbkugelige Druckdetails"),
        ("coal3_schlitz_hoehe", 1, "Coal3: Höhe der waagerechten Lüftungsschlitze im Seitenfeld"),
        ("coal3_schlitz_abstand", 2.2, "Coal3: Abstand der Schlitzreihen; Steg = Abstand - Höhe (≥ 1 mm)"),
        ("coal3_schlitz_laenge", 12, "Coal3: Länge eines Schlitzes (kurz = saubere Brücke beim Druck)"),
        ("coal3_schlitz_steg", 1.5, "Coal3: senkrechter Steg zwischen den Schlitzspalten"),
        ("coal3_schlitz_anstieg", 1.5, "Coal3: Schlitz ist innen so viel höher als außen; Wasser läuft nach außen ab"),
    ]),
    ("Kohlemodul 3 Deckel — eingelegt, Kohle bündig mit der Oberkante", (0.22, 0.30, 0.38), [
        ("coal3_deckel_dicke", 1.2, "Coal3: Stärke der Deckelplatte unter der Kohle"),
        ("coal3_deckel_spiel", 0.25, "Coal3: Luft je Seite zwischen Deckel und Innenwand"),
        ("coal3_auflage", 1.5, "Coal3: Breite der 45°-Auflageleiste innen, auf der der Deckel liegt"),
        ("coal3_kohle_ueber", 0, "Coal3: höchstes Kohlestück über der Oberkante; 0 = bündig, negativ = tiefer"),
        ("coal3_kohle_hoehe", f"{coal_height:.3f}","Coal3: Höhe der Kohle über der Deckelplatte; vom Skript gemessen, nicht ändern"),
    ]),
    ("Kohlemodul 3 — abgeleitet, Formeln beibehalten", (0.30, 0.36, 0.42), [
        ("coal3_kasten_laenge", "=laenge-2*coal3_einzug", "Coal3: obere Außenlänge"),
        ("coal3_kasten_breite", "=breite-2*coal3_einzug", "Coal3: obere Außenbreite"),
        ("coal3_knie", "=modulplatte_dicke+coal3_hoehe*coal3_knick_anteil",
         "Coal3: Höhe des Wandknicks über Z=0"),
        ("coal3_oben", "=modulplatte_dicke+coal3_hoehe", "Coal3: Höhe des Hauptgurtes über Z=0"),
        ("coal3_oberkante", "=coal3_oben+coal3_rand_hoehe", "Coal3: Gesamthöhe des Druckteils"),
        ("coal3_innen_laenge", "=coal3_kasten_laenge-2*coal3_wand",
         "Coal3: freie obere Öffnung für spätere Kohleladung"),
        ("coal3_innen_breite", "=coal3_kasten_breite-2*coal3_wand",
         "Coal3: freie obere Öffnung für spätere Kohleladung"),
        ("coal3_deckel_oben", "=coal3_oberkante+coal3_kohle_ueber-coal3_kohle_hoehe",
         "Coal3: Oberseite der Deckelplatte"),
        ("coal3_deckel_unten", "=coal3_deckel_oben-coal3_deckel_dicke",
         "Coal3: Unterseite der Deckelplatte = Oberkante der Auflageleiste"),
        ("coal3_deckel_laenge", "=coal3_innen_laenge-2*coal3_deckel_spiel", "Coal3: Deckellänge"),
        ("coal3_deckel_breite", "=coal3_innen_breite-2*coal3_deckel_spiel", "Coal3: Deckelbreite"),
        ("coal3_schlitz_feld", f"={VENT_ROWS - 1}*coal3_schlitz_abstand+coal3_schlitz_hoehe+coal3_schlitz_anstieg",
         "Coal3: Höhe des Schlitzfeldes durch die Wand"),
        ("coal3_schlitz_unten", "=(coal3_knie+coal3_deckel_unten-coal3_auflage-coal3_schlitz_feld)/2",
         "Coal3: Unterkante der untersten Schlitzreihe außen; Feld mittig zwischen Knick und Auflage"),
    ]),
]
cells = {}
row = first_row
for title, color, entries in categories:
    sheet.set(f"A{row}", title)
    sheet.mergeCells(f"A{row}:C{row}")
    sheet.setBackground(f"A{row}:C{row}", color)
    sheet.setForeground(f"A{row}:C{row}", (1.0, 1.0, 1.0))
    row += 1
    for alias, value, description in entries:
        assert not sheet.getCellFromAlias(alias), f"Parameter already exists: {alias}"
        cells[alias] = f"B{row}"
        sheet.set(f"A{row}", alias)
        sheet.setAlias(f"B{row}", alias)
        sheet.set(f"B{row}", str(value))
        sheet.set(f"C{row}", description)
        row += 1
    row += 1
doc.recompute()

L, W = C + "kasten_laenge", C + "kasten_breite"
F, K, T = P + "modulplatte_dicke", C + "knie", C + "oben"
V, TH, R = C + "verjuengung", C + "wand", C + "relief"
B, G = C + "traeger_breite", C + "gurt_hoehe"

# Hopper shell: two native tapered solids, hollowed from the floor upwards.
floor = box("MountingFloor", P + "laenge", P + "breite", F,
            f"-{P}laenge/2", f"-{P}breite/2", 0)
lower = wedge("LowerShell", f"-{L}/2+{V}", f"{L}/2-{V}", f"{F}-0.05", K,
              f"-{W}/2+{V}", f"-{W}/2", f"{W}/2-{V}", f"{W}/2",
              xt0=f"-{L}/2", xt1=f"{L}/2")
upper = box("UpperShell", L, W, f"{T}-{K}+0.05", f"-{L}/2", f"-{W}/2", f"{K}-0.05")
lower_void = wedge("LowerCavity", f"-{L}/2+{V}+{TH}", f"{L}/2-{V}-{TH}", F, K,
                   f"-{W}/2+{V}+{TH}", f"-{W}/2+{TH}",
                   f"{W}/2-{V}-{TH}", f"{W}/2-{TH}",
                   xt0=f"-{L}/2+{TH}", xt1=f"{L}/2-{TH}")
upper_void = box("UpperCavity", f"{L}-2*{TH}", f"{W}-2*{TH}",
                 f"{T}-{K}+1", f"-{L}/2+{TH}", f"-{W}/2+{TH}", f"{K}-0.01")
shell = cut("HopperWalls", fuse("ShellOutside", [lower, upper]),
            fuse("ShellInside", [lower_void, upper_void]))
parts = [floor, shell]
vents = []
parts.append(ring("BottomBand", P + "laenge", P + "breite", G,
                  f"{F}-0.1", f"{C}einzug+{V}+0.3"))
parts.append(ring("TopBand", P + "laenge", P + "breite", G,
                  f"{T}-{G}", f"{C}einzug+{TH}"))
# Raised open rim, shallow outer groove suggested by a second narrow top lip.
parts.append(ring("RaisedRim", L, W, f"{C}rand_hoehe+0.1", f"{T}-0.1", C + "rand_wand"))
parts.append(ring("RimLip", f"{L}+0.8", f"{W}+0.8", "0.8",
                  f"{C}oberkante-0.8", f"{C}rand_wand+0.4"))

for face in ("S", "N", "E", "W"):
    side = face in ("S", "N")
    span, radius = (L, f"{W}/2") if side else (W, f"{L}/2")
    positions = [f"-{span}/2+{B}/2", f"{span}/2-{B}/2"]
    if side:
        positions += [f"{f}*{span}" for f in (-0.34, -0.18, 0.18, 0.34)]
    else:
        positions += [f"{span}*{(i-(FIN_COUNT-1)/2)/(FIN_COUNT+1):.6f}"
                      for i in range(FIN_COUNT)]
    for i, x in enumerate(positions):
        width = B if side or i < 2 else C + "rippe_breite"
        # Corner posts follow the end-wall taper as well as the side taper.
        bottom_x = f"({x})+{V}" if i == 0 else f"({x})-{V}" if i == 1 else x
        parts.append(wedge(f"{face}PostLower{i}", f"({bottom_x})-{width}/2",
                           f"({bottom_x})+{width}/2", f"{F}-0.02", K,
                           f"{radius}-{V}-0.15", f"{radius}-0.15",
                           f"{radius}-{V}+{R}", f"{radius}+{R}", face,
                           f"({x})-{width}/2", f"({x})+{width}/2"))
        parts.append(wedge(f"{face}PostUpper{i}", f"({x})-{width}/2", f"({x})+{width}/2",
                           f"{K}-0.01", T, f"{radius}-0.15", f"{radius}-0.15",
                           f"{radius}+{R}", f"{radius}+{R}", face))
    if side:
        # Sweeping diagonals in both end panels; the centre stays clear for a sign.
        for sign in (-1, 1):
            x0 = f"{sign}*({span}/2-{V}-{B})"
            x1 = f"{sign}*{span}*0.18"
            xk = f"({x0})+(({x1})-({x0}))*{C}knick_anteil"
            bw = C + "strebe_breite"
            for label, a, b, za, zb, da in (
                ("Lower", x0, xk, F, K, f"{radius}-{V}"),
                ("Upper", xk, x1, K, f"{T}-{G}", radius),
            ):
                parts.append(wedge(f"{face}Brace{sign+1}{label}", f"({a})-{bw}/2",
                                   f"({a})+{bw}/2", za, zb, f"{da}-0.1", f"{radius}-0.1",
                                   f"{da}+{R}", f"{radius}+{R}", face,
                                   f"({b})-{bw}/2", f"({b})+{bw}/2"))
        # Horizontal vent slots between the inner posts (where a nameplate would
        # sit): short slots in columns, each rising inwards so rain runs out.
        sl, rise = C + "schlitz_laenge", C + "schlitz_anstieg"
        for j in range(VENT_ROWS):
            z_out = f"{C}schlitz_unten+{j}*{C}schlitz_abstand"  # outer face, slot bottom
            for c in range(VENT_COLUMNS):
                x = f"({sl}+{C}schlitz_steg)*{c - (VENT_COLUMNS - 1) / 2:g}"
                vents.append(rising_slot(f"{face}Vent{j}{c}", f"({x})-{sl}/2", f"({x})+{sl}/2",
                                         f"{radius}-{TH}-0.5", f"{radius}+0.5",
                                         f"({z_out})-{rise}*0.5/{TH}", C + "schlitz_hoehe",
                                         f"{rise}*({TH}+1)/{TH}", face))
        # Printable half-domed rivets on the upper and lower panel edges.
        for level, z in (("Top", f"{T}-{G}-1.5"), ("Knee", f"{K}+1.5")):
            for i in range(RIVETS_PER_ROW):
                rivet = doc.addObject("Part::Sphere", f"Coal3{face}Rivet{level}{i}")
                rivet.setExpression("Radius", f"{C}niet_d/2")
                rivet.setExpression("Placement.Base.x", f"{L}*{(i/(RIVETS_PER_ROW-1)-0.5)*0.92:.6f}")
                rivet.setExpression("Placement.Base.y", f"{1 if face == 'N' else -1}*({radius}-0.08)")
                rivet.setExpression("Placement.Base.z", z)
                parts.append(rivet)
    # Inner 45° ledge carrying the drop-in lid; chamfered underneath, prints unsupported.
    lu, ledge = C + "deckel_unten", C + "auflage"
    parts.append(wedge(f"{face}LidLedge", f"-({span})/2+{TH}", f"({span})/2-{TH}",
                       f"{lu}-{ledge}", lu, f"{radius}-{TH}", f"{radius}-{TH}-{ledge}",
                       f"{radius}-{TH}+0.3", f"{radius}-{TH}+0.3", face))
    # Short straps across the raised rim, rooted in the top band.
    strap_positions = (-0.32, 0, 0.32) if side else (-0.25, 0.25)
    for i, factor in enumerate(strap_positions):
        x = f"{span}*{factor}"
        parts.append(wedge(f"{face}RimStrap{i}", f"({x})-1", f"({x})+1",
                           f"{T}-0.3", C + "oberkante",
                           f"{radius}-0.1", f"{radius}-0.1",
                           f"{radius}+{R}", f"{radius}+0.6", face))

holes = []
for sx in (-1, 1):
    for sy in (-1, 1):
        hole = doc.addObject("Part::Cylinder", f"Coal3MountHole{sx+1}{sy+1}")
        hole.setExpression("Radius", f"{P}modul_bohrung_d/2")
        hole.setExpression("Height", f"{F}+0.2")
        hole.setExpression("Placement.Base.x", f"{sx}*{P}radstand/2")
        hole.Placement.Base.y = sy * 6
        hole.Placement.Base.z = -0.1
        holes.append(hole)
final = doc.addObject("Part::Cut", "ModuleCoal3")
final.Base = fuse("BodyAndFrame", parts)
final.Tool = fuse("HolesAndVents", holes + vents)
final.Label = "ModuleCoal3"
final.addProperty("App::PropertyString", "PrintOrientation", "Printing")
final.PrintOrientation = "Flat floor on bed (Z=0); open top upwards"
final.addProperty("App::PropertyString", "Mounting", "Printing")
final.Mounting = "4 M2 screws from inside, at (±radstand/2, ±6); use existing axle-bridge pilots"

# Drop-in lid: a plate on the inner ledge, carrying the existing coal load so
# that its highest lump sits coal3_kohle_ueber above the rim top (default flush).
lid = doc.addObject("Part::Box", "ModuleCoal3Lid")
for prop, value in (("Length", C + "deckel_laenge"), ("Width", C + "deckel_breite"),
                    ("Height", C + "deckel_dicke"),
                    ("Placement.Base.x", f"-{C}deckel_laenge/2"),
                    ("Placement.Base.y", f"-{C}deckel_breite/2"),
                    ("Placement.Base.z", C + "deckel_unten")):
    lid.setExpression(prop, value)
lid.Label = "ModuleCoal3Lid"
lid.addProperty("App::PropertyString", "PrintOrientation", "Printing")
lid.PrintOrientation = "Plate down, coal up; exported together with Coal3Load as ModuleCoal3Lid.stl"
doc.recompute()
# The coal footprint is a mesh: scale the CoalLoad copy once to the lid plate.
scale = App.Matrix()
scale.scale((val("coal3_deckel_laenge") / 2 - COAL_MARGIN) / max(abs(coal_bb.XMin), coal_bb.XMax),
            (val("coal3_deckel_breite") / 2 - COAL_MARGIN) / max(abs(coal_bb.YMin), coal_bb.YMax), 1)
coal_src.transformGeometry(scale)
coal = doc.addObject("Mesh::Feature", "Coal3Load")
coal.Mesh = coal_src
coal.Label = "Coal3Load"
coal.setExpression(".Placement.Base.z", f"{C}deckel_oben-{coal_bottom + 0.05:.4f}")
created =[obj for obj in doc.Objects if obj.Name not in existing_names]

validate("Default")
if os.environ.get("COAL3_FULL_CHECK") == "1":
    for label, changes in (
        ("Short body", {"coal3_hoehe": 40}),
        ("Tall / thicker walls", {"coal3_hoehe": 75, "coal3_wand": 2.4}),
        ("Coal 2 mm proud, thicker lid", {"coal3_kohle_ueber": 2, "coal3_deckel_dicke": 1.6}),
        ("Frame adjustments", {"coal3_hoehe": 48, "coal3_verjuengung": 3,
                               "coal3_knick_anteil": 0.35, "coal3_traeger_breite": 3,
                               "coal3_rand_hoehe": 5, "coal3_schlitz_abstand": 2.6,
                               "coal3_schlitz_laenge": 10, "coal3_schlitz_anstieg": 1}),
    ):
        originals = {key: sheet.getContents(cells[key]) for key in changes}
        for key, value in changes.items():
            sheet.set(cells[key], str(value))
        validate(label, check_coal=all(k in ("coal3_hoehe", "coal3_kohle_ueber",
                                             "coal3_deckel_dicke") for k in changes))
        for key, value in originals.items():
            sheet.set(cells[key], value)
    validate("Restored defaults")

# Existing parameter contents and base/module shapes must survive the addition.
for address, content in original_cells.items():
    assert sheet.getContents(address) == content, f"Existing cell changed: {address}"
for name, before in preserved.items():
    after = doc.getObject(name).Shape
    if before.Solids:
        assert before.cut(after).Volume < 1e-6 and after.cut(before).Volume < 1e-6, f"{name} changed"
    else:
        assert abs(before.Area - after.Area) < 1e-6, f"{name} changed"
print(f"Existing Params and {len(preserved)} existing parts preserved", flush=True)

mesh = MeshPart.meshFromShape(Shape=final.Shape, LinearDeflection=0.08,
                              AngularDeflection=0.20, Relative=False)
assert mesh.isSolid(), "Export mesh is not closed"
assert mesh.countComponents() == 1, "Export mesh contains disconnected components"
mesh.write(os.path.join(OUT, "ModuleCoal3.stl"))
Part.export([final], os.path.join(OUT, "ModuleCoal3.step"))
# Lid = plate + coal in one file (two overlapping shells; slicers merge them).
lid_mesh = MeshPart.meshFromShape(Shape=lid.Shape, LinearDeflection=0.08,
                                  AngularDeflection=0.20, Relative=False)
coal_world = coal.Mesh.copy()
coal_world.Placement = App.Placement()
coal_world.transformGeometry(coal.Placement.toMatrix())
lid_mesh.addMesh(coal_world)
lid_mesh.write(os.path.join(OUT, "ModuleCoal3Lid.stl"))

# Keep the visibility of existing objects; show the finished body, lid and coal.
for obj in created:
    obj.Visibility = obj in (final, lid, coal)
    if obj.ViewObject:
        obj.ViewObject.Visibility = obj.Visibility
for name, shown in visible_before.items():
    doc.getObject(name).Visibility = shown
if final.ViewObject:
    final.ViewObject.ShapeColor = (0.43, 0.47, 0.49)
    final.ViewObject.LineColor = (0.16, 0.18, 0.19)
    lid.ViewObject.ShapeColor = (0.43, 0.47, 0.49)
    coal.ViewObject.ShapeColor = (0.08, 0.08, 0.08)
doc.recompute()
assert fingerprint(SOURCE) == source_hash, "WagonBase.FCStd changed on disk during the build; not overwriting it"
doc.save()
print("Added ModuleCoal3 + lid to WagonBase.FCStd; saved ModuleCoal3.stl/.step and ModuleCoal3Lid.stl", flush=True)
