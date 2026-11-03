# Add the styled shot-glass module to the EXISTING WagonBase document.
# Run: freecadcmd module_shot2.py
# Uses native expression-driven features; no Python proxy is needed on reopen.
# Existing chassis geometry, modules and parameter values are preserved.

import hashlib
import math
import os
import re
import xml.etree.ElementTree as ET
import zipfile

import FreeCAD as App
import MeshPart
import Part

OUT = os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals() else os.getcwd()
SOURCE = os.path.join(OUT, "WagonBase.FCStd")
P = "Params."


def fingerprint(path):
    with open(path, "rb") as stream:
        return hashlib.sha256(stream.read()).hexdigest()


def box(name, length, width, height, x, y, z):
    obj = doc.addObject("Part::Box", "Shot2" + name)
    for prop, value in zip(("Length", "Width", "Height", "Placement.Base.x",
                            "Placement.Base.y", "Placement.Base.z"),
                           (length, width, height, x, y, z)):
        obj.setExpression(prop, str(value))
    return obj


def cylinder(name, radius, height, x, y, z):
    obj = doc.addObject("Part::Cylinder", "Shot2" + name)
    for prop, value in zip(("Radius", "Height", "Placement.Base.x",
                            "Placement.Base.y", "Placement.Base.z"),
                           (radius, height, x, y, z)):
        obj.setExpression(prop, str(value))
    return obj


def cone(name, r0, r1, height, x, z):
    obj = doc.addObject("Part::Cone", "Shot2" + name)
    for prop, value in zip(("Radius1", "Radius2", "Height", "Placement.Base.x",
                            "Placement.Base.z"), (r0, r1, height, x, z)):
        obj.setExpression(prop, str(value))
    return obj


def fuse(name, objects):
    obj = doc.addObject("Part::MultiFuse", "Shot2" + name)
    obj.Shapes = objects
    return obj


def face_box(name, face, a, width, z, height, depth, embed="0.1"):
    """Rectangular relief on one wall; all details stay within the deck edge."""
    if face in ("N", "S"):
        y = f"{P}breite/2-{P}shot2_einzug-({embed})" if face == "N" else \
            f"-{P}breite/2+{P}shot2_einzug-({depth})"
        return box(name, width, f"({depth})+({embed})", height, a, y, z)
    x = f"{P}laenge/2-{P}shot2_einzug-({embed})" if face == "E" else \
        f"-{P}laenge/2+{P}shot2_einzug-({depth})"
    return box(name, f"({depth})+({embed})", width, height, x, a, z)


def rivet(name, face, a, z):
    obj = doc.addObject("Part::Sphere", "Shot2" + name)
    obj.setExpression("Radius", f"{P}shot2_niet_d/2")
    sign = 1 if face in ("N", "E") else -1
    radius = f"{P}{'breite' if face in ('N', 'S') else 'laenge'}/2-{P}shot2_einzug"
    radial = f"{sign}*({radius}+{P}shot2_relief*0.45-0.05)"
    x, y = (a, radial) if face in ("N", "S") else (radial, a)
    for prop, value in (("Placement.Base.x", x), ("Placement.Base.y", y), ("Placement.Base.z", z)):
        obj.setExpression(prop, str(value))
    return obj


def value(alias):
    v = getattr(sheet, alias)
    return float(v.Value if hasattr(v, "Value") else v)


def validate(label):
    doc.recompute()
    for obj in created:
        assert "Invalid" not in obj.State and "Error" not in obj.State, (obj.Name, obj.State)
    shape = final.Shape
    assert shape.isValid() and len(shape.Solids) == 1, f"{label}: invalid/disconnected body"
    bb = shape.BoundBox
    for actual, expected in ((bb.XLength, value("laenge")), (bb.YLength, value("breite")),
                             (bb.ZLength, value("shot2_hoehe")), (bb.ZMin, 0)):
        assert abs(actual-expected) < 1e-6, f"{label}: incorrect envelope"
    assert shape.common(base.Shape).Volume < 1e-6, f"{label}: chassis interference"
    floor_z = value("shot2_sitz_z")
    assert floor_z > value("shot2_boden_min"), f"{label}: insufficient material below glasses"
    assert value("shot2_eintauchen") > value("glas_fase"), f"{label}: chamfer deeper than pocket"
    # Keep sufficient material between glass openings and the module edge.
    top_radius = value("shot2_sitz_r") + value("shot2_eintauchen") * math.tan(math.radians(value("glas_konus")))
    entry_radius = top_radius + value("glas_fase") * (1-math.tan(math.radians(value("glas_konus"))))
    assert value("glas_abstand") + entry_radius + 2 <= value("laenge")/2
    assert entry_radius + 2 <= value("breite")/2-value("shot2_einzug")
    assert 2*entry_radius+2 <= value("glas_abstand")
    # Test the actual tapered cavities and the supporting floor rings.
    for sx in (-1, 0, 1):
        x = sx*value("glas_abstand")
        pocket = Part.makeCone(value("shot2_sitz_r")-0.02, top_radius-0.02,
                               value("shot2_eintauchen"), App.Vector(x, 0, floor_z+0.001))
        assert shape.common(pocket).Volume < 1e-6, f"{label}: blocked glass pocket"
        ring_point = App.Vector(x, value("shot2_sitz_r")-0.5, floor_z-0.1)
        assert shape.isInside(ring_point, 1e-6, True), f"{label}: unsupported glass"
    for sx, sy in [(sx, sy) for sx in (-1, 1) for sy in (-1, 1)] + [(0, -1), (0, 1)]:
        x = value("mitte_x") if sx == 0 else sx*(value("laenge")/2-5.5)
        y = sy*(value("breite")/2-5.5)
        bore = Part.makeCylinder(value("vorbohrung_d")/2-0.01,
                                 value("shot_schraube_tiefe"), App.Vector(x, y, 0))
        assert shape.common(bore).Volume < 1e-6, f"{label}: blocked mounting pilot"
        assert shape.isInside(App.Vector(x, y, value("shot_schraube_tiefe")+0.2), 1e-6, True), \
            f"{label}: mounting pilot is not blind"
    # Identical holes under all three glasses, ending on the closed bottom plate.
    radius, plate = value("shot2_loch_d")/2, value("shot2_gurt")
    assert 0 < radius < value("shot2_sitz_r")-1, f"{label}: no seat ring left"
    assert floor_z > plate+0.5, f"{label}: glass seat below bottom plate"
    for sx in (-1, 0, 1):
        x = sx*value("glas_abstand")
        drain = Part.makeCylinder(radius-0.01, floor_z-plate-0.1, App.Vector(x, 0, plate+0.05))
        assert shape.common(drain).Volume < 1e-6, f"{label}: blocked drain"
        assert shape.isInside(App.Vector(x, 0, plate/2), 1e-6, True), f"{label}: bottom plate open"
    print(f"{label}: valid single solid, {bb.XLength:g} x {bb.YLength:g} x {bb.ZLength:g} mm; "
          "3 glass seats, 6 blind mounting pilots and clear drains", flush=True)


source_hash = fingerprint(SOURCE)
with zipfile.ZipFile(SOURCE) as archive:
    root = ET.fromstring(archive.read("Document.xml"))
    params_xml = root.find("./ObjectData/Object[@name='Params']")
    original_cells = {c.get("address"): c.get("content", "") for c in params_xml.iter("Cell")}
    first_row = max(int(re.search(r"\d+", a).group()) for a in original_cells) + 2
doc = App.openDocument(SOURCE)
sheet = doc.getObject("Params")
base = doc.getObject("WagonBase")
if doc.getObject("ModuleShot2") or any(o.Name.startswith("Shot2") for o in doc.Objects):
    raise RuntimeError("ModuleShot2 already exists. Edit Params in WagonBase.FCStd; do not reinstall over existing work.")
base_before = base.Shape.copy()
shot_before = doc.getObject("ModuleShot").Shape.copy()
existing_names = {obj.Name for obj in doc.Objects}

rows = [
    ("shot2_hoehe", 30, "Shot2: Gesamthöhe über dem Fahrgestell; empfohlen 24–40 mm"),
    ("shot2_eintauchen", 20, "Shot2: wie tief das Glas im Modul steckt (mm ab Oberkante); max. shot2_hoehe - shot2_boden_min"),
    ("shot2_einzug", 1.8, "Shot2: Einzug der Seitenwände gegenüber Deckel/Boden"),
    ("shot2_gurt", 2.4, "Shot2: Stärke der oberen Deckplatte und des unteren Gurtes"),
    ("shot2_relief", 1.2, "Shot2: maximale äußere Rippenhöhe; kleiner als shot2_einzug"),
    ("shot2_rippe", 1.1, "Shot2: Breite jeder Rippe eines Rippenpaares"),
    ("shot2_rippenabstand", 2.8, "Shot2: Mittenabstand der gepaarten Rippen"),
    ("shot2_eckband", 4.5, "Shot2: Breite der genieteten Eckbänder"),
    ("shot2_niet_d", 1.2, "Shot2: Durchmesser der halbkugeligen Nieten"),
    ("shot2_griff", 1.4, "Shot2: Breite der druckbaren erhabenen Griffkonturen"),
    ("shot2_schild_breite", 29, "Shot2: Breite der leeren Beschriftungsfelder"),
    ("shot2_schild_hoehe", 12, "Shot2: Höhe der leeren Beschriftungsfelder"),
    ("shot2_boden_min", 3, "Shot2: minimale Materialstärke unter den Glasböden (Prüfung)"),
    ("shot2_sitz_z", "=shot2_hoehe-shot2_eintauchen", "Shot2: abgeleitete Höhe der Glasauflage"),
    ("shot2_sitz_r", "=(glas_d+glas_spiel)/2+(shot_boden-glas_d_hoehe)*tan(glas_konus)",
     "Shot2: Radius unten, übernimmt die vorhandene konische Glaspassung"),
    ("shot2_loch_d", 26, "Shot2: Durchmesser der Durchgangslöcher unter allen drei Gläsern; Rest bleibt als Auflagering"),
]
sheet.set(f"A{first_row}", "Shotmodul 2 — Wagenkasten nach Bild")
sheet.mergeCells(f"A{first_row}:C{first_row}")
sheet.setBackground(f"A{first_row}:C{first_row}", (0.22, 0.34, 0.25))
sheet.setForeground(f"A{first_row}:C{first_row}", (1., 1., 1.))
cells = {}
for row, (alias, default, description) in enumerate(rows, first_row+1):
    assert not sheet.getCellFromAlias(alias), f"Parameter already exists: {alias}"
    cells[alias] = f"B{row}"
    sheet.set(f"A{row}", alias)
    sheet.setAlias(f"B{row}", alias)
    sheet.set(f"B{row}", str(default))
    sheet.set(f"C{row}", description)
doc.recompute()

L, W, H = P+"laenge", P+"breite", P+"shot2_hoehe"
E, G, R = P+"shot2_einzug", P+"shot2_gurt", P+"shot2_relief"
rib, spacing, corner = P+"shot2_rippe", P+"shot2_rippenabstand", P+"shot2_eckband"
parts = [
    box("Body", f"{L}-2*{E}", f"{W}-2*{E}", H, f"-{L}/2+{E}", f"-{W}/2+{E}", 0),
    box("BottomBand", L, W, G, f"-{L}/2", f"-{W}/2", 0),
    box("TopDeck", L, W, G, f"-{L}/2", f"-{W}/2", f"{H}-{G}"),
]
for face in ("S", "N", "E", "W"):
    side = face in ("S", "N")
    span = L if side else W
    for sign in (-1, 1):
        a = f"-{span}/2+{E}" if sign < 0 else f"{span}/2-{E}-{corner}"
        parts.append(face_box(f"{face}Corner{sign+1}", face, a, corner, f"{G}-0.1",
                              f"{H}-2*{G}+0.2", f"{R}*0.45"))
        for i in range(4):
            parts.append(rivet(f"{face}CornerRivet{sign+1}{i}", face, f"({a})+{corner}/2",
                                f"{G}+2.4+({H}-2*{G}-4.8)*{i}/3"))
    if side:
        # Four pairs of slender ribs, framing three broad panels.
        for i, x in enumerate((f"-{L}/2+14", f"-{P}glas_abstand/2",
                               f"{P}glas_abstand/2", f"{L}/2-14")):
            for sign in (-1, 1):
                parts.append(face_box(f"{face}TwinRib{i}{sign+1}", face,
                                      f"({x})+{sign}*{spacing}/2-{rib}/2", rib,
                                      f"{G}-0.05", f"{H}-2*{G}+0.1", R))
        # Blank raised label panels occupy the left and right bays.
        for sign in (-1, 1):
            x = f"{sign}*({L}/2-14+{P}glas_abstand/2)/2"
            parts.append(face_box(f"{face}BlankPanel{sign+1}", face,
                                  f"({x})-{P}shot2_schild_breite/2", P+"shot2_schild_breite",
                                  f"({H}-{P}shot2_schild_hoehe)/2", P+"shot2_schild_hoehe", "0.3"))
        parts.append(face_box(face+"LowerHinge", face, "-9", "18", G, "3.4", f"{R}*0.45"))
        for i in range(4):
            parts.append(rivet(f"{face}HingeRivet{i}", face, str(-6+i*4), f"{G}+1.7"))
    else:
        # Two horizontal end-wall rails, with tall grab-handle outlines at the sides.
        for i, fraction in enumerate((0.38, 0.65)):
            parts.append(face_box(f"{face}CrossRail{i}", face, f"-{W}/2+10",
                                  f"{W}-20", f"{H}*{fraction}", P+"shot2_griff", R))
        for sign in (-1, 1):
            a = f"{sign}*({W}/2-8)-1.6"
            z, height, rail = f"{H}*0.20", f"{H}*0.60", P+"shot2_griff"
            for i, (aa, width, zz, hh) in enumerate((
                (a, rail, z, height), (f"({a})+3.2", rail, z, height),
                (a, f"3.2+{rail}", z, rail),
                (a, f"3.2+{rail}", f"({z})+({height})-{rail}", rail),
            )):
                parts.append(face_box(f"{face}Grab{sign+1}{i}", face, aa, width, zz, hh, R))

floor_z, floor_r, depth = P+"shot2_sitz_z", P+"shot2_sitz_r", P+"shot2_eintauchen"
negatives = []
for sign in (-1, 0, 1):
    x = f"{sign}*{P}glas_abstand"
    negatives.append(cone(f"GlassPocket{sign+1}", floor_r,
                          f"{floor_r}+({depth}+1)*tan({P}glas_konus)", f"{depth}+1", x, floor_z))
    chamfer_r = f"{floor_r}+({depth}-{P}glas_fase)*tan({P}glas_konus)"
    negatives.append(cone(f"GlassLeadIn{sign+1}", chamfer_r,
                          f"({chamfer_r})+2*{P}glas_fase", f"2*{P}glas_fase", x, f"{H}-{P}glas_fase"))
    # Same hole under all three glasses, from the seat down to the bottom plate.
    negatives.append(cylinder(f"Drain{sign+1}", f"{P}shot2_loch_d/2",
                              f"{floor_z}-{G}+0.2", x, 0, G))
for sx, sy in [(sx, sy) for sx in (-1, 1) for sy in (-1, 1)] + [(0, -1), (0, 1)]:
    x = P+"mitte_x" if sx == 0 else f"{sx}*({L}/2-5.5)"
    negatives.append(cylinder(f"MountPilot{sx+1}{sy+1}", f"{P}vorbohrung_d/2",
                              f"{P}shot_schraube_tiefe+0.1", x, f"{sy}*({W}/2-5.5)", "-0.1"))
final = doc.addObject("Part::Cut", "ModuleShot2")
final.Base = fuse("Frame", parts)
final.Tool = fuse("PocketsAndDrains", negatives)
final.Label = "ModuleShot2"
final.addProperty("App::PropertyString", "Mounting", "Printing")
final.Mounting = "Same six underside M2 pilots as ModuleShot; flat underside at Z=0"
final.addProperty("App::PropertyString", "GlassFit", "Printing")
final.GlassFit = "Existing glas_* parameters; shot2_eintauchen controls seating depth independently of body height"
created = [obj for obj in doc.Objects if obj.Name not in existing_names]
validate("Default")
if os.environ.get("SHOT2_FULL_CHECK") == "1":
    for label, changes in (
        ("Low body", {"shot2_hoehe": 24}),
        ("Tall body / deeper seats", {"shot2_hoehe": 40, "shot2_eintauchen": 16}),
        ("Frame adjustments", {"shot2_hoehe": 32, "shot2_gurt": 3,
                               "shot2_rippe": 1.3, "shot2_relief": 1.4, "shot2_schild_hoehe": 14}),
    ):
        originals = {alias: sheet.getContents(cells[alias]) for alias in changes}
        for alias, v in changes.items():
            sheet.set(cells[alias], str(v))
        validate(label)
        for alias, v in originals.items():
            sheet.set(cells[alias], v)
    validate("Restored defaults")

# Existing parameter contents and base/module shapes must survive the addition.
for address, content in original_cells.items():
    assert sheet.getContents(address) == content, f"Existing cell changed: {address}"
for name, before in (("WagonBase", base_before), ("ModuleShot", shot_before)):
    after = doc.getObject(name).Shape
    assert before.cut(after).Volume < 1e-6 and after.cut(before).Volume < 1e-6, f"{name} changed"
print("Existing Params, WagonBase and ModuleShot preserved", flush=True)
mesh = MeshPart.meshFromShape(Shape=final.Shape, LinearDeflection=0.08,
                              AngularDeflection=0.20, Relative=False)
assert mesh.isSolid() and mesh.countComponents() == 1, "Export must be a closed, connected mesh"
mesh.write(os.path.join(OUT, "ModuleShot2.stl"))
Part.export([final], os.path.join(OUT, "ModuleShot2.step"))
for obj in doc.Objects:
    obj.Visibility = obj.Name in ("WagonBase", "ModuleShot2")
    if obj.ViewObject:
        obj.ViewObject.Visibility = obj.Visibility
if final.ViewObject:
    final.ViewObject.ShapeColor = (0.32, 0.42, 0.29)
if App.GuiUp:
    import FreeCADGui as Gui
    Gui.activeDocument().activeView().viewAxonometric()
    Gui.activeDocument().activeView().fitAll()
doc.recompute()
assert fingerprint(SOURCE) == source_hash, "WagonBase.FCStd changed on disk during the build; not overwriting it"
doc.save()
print("Added ModuleShot2 to WagonBase.FCStd; saved ModuleShot2.stl and ModuleShot2.step", flush=True)
