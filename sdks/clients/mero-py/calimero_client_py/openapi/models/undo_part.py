from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.undo_part_lost_writes_type_0 import UndoPartLostWritesType0
    from ..models.package_ref import PackageRef
    from ..models.undo_part_restore_point_type_0 import UndoPartRestorePointType0
    from ..models.undo_part_unavailable_type_0 import UndoPartUnavailableType0


T = TypeVar("T", bound="UndoPart")


@_attrs_define
class UndoPart:
    """One app an undo reverts: the release it leaves (`from`) and returns to (`to`, `toBundle`), when it was upgraded and
    by whom (`auto`: on its own), until when it may be undone, its restore point, and what the undo loses (`lostWrites`:
    the app group's entries since the restore point, its last write, and `any`: whether it wrote since the upgrade;
    `null` while the restore point's node does not answer, which `unavailable` then says).

        Attributes:
            app (str):
            from_ (Union['PackageRef', None]):
            to (Union['PackageRef', None]):
            to_bundle (str):
            upgraded_at (int):
            upgraded_by (str):
            auto (bool):
            undo_until (int):
            restore_point (Union['UndoPartRestorePointType0', None]):
            undone_at (Union[None, int]):
            lost_writes (Union['UndoPartLostWritesType0', None]):
            unavailable (Union['UndoPartUnavailableType0', None]):
    """

    app: str
    from_: Union["PackageRef", None]
    to: Union["PackageRef", None]
    to_bundle: str
    upgraded_at: int
    upgraded_by: str
    auto: bool
    undo_until: int
    restore_point: Union["UndoPartRestorePointType0", None]
    undone_at: Union[None, int]
    lost_writes: Union["UndoPartLostWritesType0", None]
    unavailable: Union["UndoPartUnavailableType0", None]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.undo_part_lost_writes_type_0 import UndoPartLostWritesType0
        from ..models.package_ref import PackageRef
        from ..models.undo_part_restore_point_type_0 import UndoPartRestorePointType0
        from ..models.undo_part_unavailable_type_0 import UndoPartUnavailableType0

        app = self.app

        from_: Union[None, dict[str, Any]]
        if isinstance(self.from_, PackageRef):
            from_ = self.from_.to_dict()
        else:
            from_ = self.from_

        to: Union[None, dict[str, Any]]
        if isinstance(self.to, PackageRef):
            to = self.to.to_dict()
        else:
            to = self.to

        to_bundle = self.to_bundle

        upgraded_at = self.upgraded_at

        upgraded_by = self.upgraded_by

        auto = self.auto

        undo_until = self.undo_until

        restore_point: Union[None, dict[str, Any]]
        if isinstance(self.restore_point, UndoPartRestorePointType0):
            restore_point = self.restore_point.to_dict()
        else:
            restore_point = self.restore_point

        undone_at: Union[None, int]
        undone_at = self.undone_at

        lost_writes: Union[None, dict[str, Any]]
        if isinstance(self.lost_writes, UndoPartLostWritesType0):
            lost_writes = self.lost_writes.to_dict()
        else:
            lost_writes = self.lost_writes

        unavailable: Union[None, dict[str, Any]]
        if isinstance(self.unavailable, UndoPartUnavailableType0):
            unavailable = self.unavailable.to_dict()
        else:
            unavailable = self.unavailable

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "from": from_,
                "to": to,
                "toBundle": to_bundle,
                "upgradedAt": upgraded_at,
                "upgradedBy": upgraded_by,
                "auto": auto,
                "undoUntil": undo_until,
                "restorePoint": restore_point,
                "undoneAt": undone_at,
                "lostWrites": lost_writes,
                "unavailable": unavailable,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.undo_part_lost_writes_type_0 import UndoPartLostWritesType0
        from ..models.package_ref import PackageRef
        from ..models.undo_part_restore_point_type_0 import UndoPartRestorePointType0
        from ..models.undo_part_unavailable_type_0 import UndoPartUnavailableType0

        d = dict(src_dict)
        app = d.pop("app")

        def _parse_from_(data: object) -> Union["PackageRef", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                from_type_0 = PackageRef.from_dict(data)

                return from_type_0
            except:  # noqa: E722
                pass
            return cast(Union["PackageRef", None], data)

        from_ = _parse_from_(d.pop("from"))

        def _parse_to(data: object) -> Union["PackageRef", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                to_type_0 = PackageRef.from_dict(data)

                return to_type_0
            except:  # noqa: E722
                pass
            return cast(Union["PackageRef", None], data)

        to = _parse_to(d.pop("to"))

        to_bundle = d.pop("toBundle")

        upgraded_at = d.pop("upgradedAt")

        upgraded_by = d.pop("upgradedBy")

        auto = d.pop("auto")

        undo_until = d.pop("undoUntil")

        def _parse_restore_point(
            data: object,
        ) -> Union["UndoPartRestorePointType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                restore_point_type_0 = UndoPartRestorePointType0.from_dict(data)

                return restore_point_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UndoPartRestorePointType0", None], data)

        restore_point = _parse_restore_point(d.pop("restorePoint"))

        def _parse_undone_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        undone_at = _parse_undone_at(d.pop("undoneAt"))

        def _parse_lost_writes(data: object) -> Union["UndoPartLostWritesType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                lost_writes_type_0 = UndoPartLostWritesType0.from_dict(data)

                return lost_writes_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UndoPartLostWritesType0", None], data)

        lost_writes = _parse_lost_writes(d.pop("lostWrites"))

        def _parse_unavailable(data: object) -> Union["UndoPartUnavailableType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                unavailable_type_0 = UndoPartUnavailableType0.from_dict(data)

                return unavailable_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UndoPartUnavailableType0", None], data)

        unavailable = _parse_unavailable(d.pop("unavailable"))

        undo_part = cls(
            app=app,
            from_=from_,
            to=to,
            to_bundle=to_bundle,
            upgraded_at=upgraded_at,
            upgraded_by=upgraded_by,
            auto=auto,
            undo_until=undo_until,
            restore_point=restore_point,
            undone_at=undone_at,
            lost_writes=lost_writes,
            unavailable=unavailable,
        )

        undo_part.additional_properties = d
        return undo_part

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
