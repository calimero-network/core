from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.undo_part import UndoPart
    from ..models.undo_view_reason_type_0 import UndoViewReasonType0


T = TypeVar("T", bound="UndoView")


@_attrs_define
class UndoView:
    """`GET /apps/{app}/undo`: whether the app's last upgrade may be undone now (else the refusal's `reason`), until when,
    and each app it reverts.

        Attributes:
            app (str):
            available (bool):
            reason (Union['UndoViewReasonType0', None]):
            apps (list['UndoPart']):
            undo_until (Union[Unset, int]):
    """

    app: str
    available: bool
    reason: Union["UndoViewReasonType0", None]
    apps: list["UndoPart"]
    undo_until: Union[Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.undo_part import UndoPart
        from ..models.undo_view_reason_type_0 import UndoViewReasonType0

        app = self.app

        available = self.available

        reason: Union[None, dict[str, Any]]
        if isinstance(self.reason, UndoViewReasonType0):
            reason = self.reason.to_dict()
        else:
            reason = self.reason

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        undo_until = self.undo_until

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "available": available,
                "reason": reason,
                "apps": apps,
            }
        )
        if undo_until is not UNSET:
            field_dict["undoUntil"] = undo_until

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.undo_part import UndoPart
        from ..models.undo_view_reason_type_0 import UndoViewReasonType0

        d = dict(src_dict)
        app = d.pop("app")

        available = d.pop("available")

        def _parse_reason(data: object) -> Union["UndoViewReasonType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                reason_type_0 = UndoViewReasonType0.from_dict(data)

                return reason_type_0
            except:  # noqa: E722
                pass
            return cast(Union["UndoViewReasonType0", None], data)

        reason = _parse_reason(d.pop("reason"))

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = UndoPart.from_dict(apps_item_data)

            apps.append(apps_item)

        undo_until = d.pop("undoUntil", UNSET)

        undo_view = cls(
            app=app,
            available=available,
            reason=reason,
            apps=apps,
            undo_until=undo_until,
        )

        undo_view.additional_properties = d
        return undo_view

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
