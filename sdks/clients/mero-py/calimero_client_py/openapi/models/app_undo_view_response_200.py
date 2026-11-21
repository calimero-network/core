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
    from ..models.app_undo_view_response_200_reason_type_0 import (
        AppUndoViewResponse200ReasonType0,
    )


T = TypeVar("T", bound="AppUndoViewResponse200")


@_attrs_define
class AppUndoViewResponse200:
    """
    Attributes:
        app (str):
        available (bool):
        reason (Union['AppUndoViewResponse200ReasonType0', None]):
        apps (list['UndoPart']):
        undo_until (Union[None, Unset, int]):
    """

    app: str
    available: bool
    reason: Union["AppUndoViewResponse200ReasonType0", None]
    apps: list["UndoPart"]
    undo_until: Union[None, Unset, int] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.undo_part import UndoPart
        from ..models.app_undo_view_response_200_reason_type_0 import (
            AppUndoViewResponse200ReasonType0,
        )

        app = self.app

        available = self.available

        reason: Union[None, dict[str, Any]]
        if isinstance(self.reason, AppUndoViewResponse200ReasonType0):
            reason = self.reason.to_dict()
        else:
            reason = self.reason

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        undo_until: Union[None, Unset, int]
        if isinstance(self.undo_until, Unset):
            undo_until = UNSET
        else:
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
        from ..models.app_undo_view_response_200_reason_type_0 import (
            AppUndoViewResponse200ReasonType0,
        )

        d = dict(src_dict)
        app = d.pop("app")

        available = d.pop("available")

        def _parse_reason(
            data: object,
        ) -> Union["AppUndoViewResponse200ReasonType0", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                reason_type_0 = AppUndoViewResponse200ReasonType0.from_dict(data)

                return reason_type_0
            except:  # noqa: E722
                pass
            return cast(Union["AppUndoViewResponse200ReasonType0", None], data)

        reason = _parse_reason(d.pop("reason"))

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = UndoPart.from_dict(apps_item_data)

            apps.append(apps_item)

        def _parse_undo_until(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        undo_until = _parse_undo_until(d.pop("undoUntil", UNSET))

        app_undo_view_response_200 = cls(
            app=app,
            available=available,
            reason=reason,
            apps=apps,
            undo_until=undo_until,
        )

        app_undo_view_response_200.additional_properties = d
        return app_undo_view_response_200

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
