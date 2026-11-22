from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.upgrade_class import UpgradeClass
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.update_reason_type_4 import UpdateReasonType4
    from ..models.update_reason_type_3 import UpdateReasonType3
    from ..models.update_reason_type_1 import UpdateReasonType1
    from ..models.update_reason_type_6 import UpdateReasonType6
    from ..models.package_ref import PackageRef
    from ..models.update_reason_type_0 import UpdateReasonType0
    from ..models.update_reason_type_5 import UpdateReasonType5
    from ..models.update_reason_type_2 import UpdateReasonType2


T = TypeVar("T", bound="Upgrade")


@_attrs_define
class Upgrade:
    """The last upgrade tried on an app, done or failed: `from` and `to` are bundle hashes, `by` the account (or
    `operator`), `run` the organization-wide run it was part of, `apiKey` the upgrade key that made it. `class` and
    `reasons`: what the update classifier said of it, judged again where it applied (absent before cluster version
    1.0.0-rc.34, for a raw bundle and a failed upgrade); `approvedBy`: the first approver of a red update whose policy
    asks for a second. `auto`: it applied on its own under the app's update policy (`by` is `policy:auto-update`);
    `undo`: it undid the upgrade before it (`fromRelease` the release undone).

        Attributes:
            from_ (str):
            to (str):
            at (int):
            by (str):
            ok (bool):
            error (Union[Unset, Any]): Free-form JSON: its shape is not this API's (see the description around it for whose
                it is).
            run (Union[Unset, str]):
            api_key (Union[Unset, str]):
            from_release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-
                service bundle, `compute` (absent: false) marks the service's compute component.
            to_release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-
                service bundle, `compute` (absent: false) marks the service's compute component.
            class_ (Union[Unset, UpgradeClass]):
            reasons (Union[Unset, list[Union['UpdateReasonType0', 'UpdateReasonType1', 'UpdateReasonType2',
                'UpdateReasonType3', 'UpdateReasonType4', 'UpdateReasonType5', 'UpdateReasonType6']]]):
            approved_by (Union[Unset, str]):
            auto (Union[Unset, bool]):
            undo (Union[Unset, bool]):
    """

    from_: str
    to: str
    at: int
    by: str
    ok: bool
    error: Union[Unset, Any] = UNSET
    run: Union[Unset, str] = UNSET
    api_key: Union[Unset, str] = UNSET
    from_release: Union[Unset, "PackageRef"] = UNSET
    to_release: Union[Unset, "PackageRef"] = UNSET
    class_: Union[Unset, UpgradeClass] = UNSET
    reasons: Union[
        Unset,
        list[
            Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
            ]
        ],
    ] = UNSET
    approved_by: Union[Unset, str] = UNSET
    auto: Union[Unset, bool] = UNSET
    undo: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.package_ref import PackageRef
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2

        from_ = self.from_

        to = self.to

        at = self.at

        by = self.by

        ok = self.ok

        error = self.error

        run = self.run

        api_key = self.api_key

        from_release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.from_release, Unset):
            from_release = self.from_release.to_dict()

        to_release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.to_release, Unset):
            to_release = self.to_release.to_dict()

        class_: Union[Unset, str] = UNSET
        if not isinstance(self.class_, Unset):
            class_ = self.class_.value

        reasons: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.reasons, Unset):
            reasons = []
            for reasons_item_data in self.reasons:
                reasons_item: dict[str, Any]
                if isinstance(reasons_item_data, UpdateReasonType0):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType1):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType2):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType3):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType4):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType5):
                    reasons_item = reasons_item_data.to_dict()
                else:
                    reasons_item = reasons_item_data.to_dict()

                reasons.append(reasons_item)

        approved_by = self.approved_by

        auto = self.auto

        undo = self.undo

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "from": from_,
                "to": to,
                "at": at,
                "by": by,
                "ok": ok,
            }
        )
        if error is not UNSET:
            field_dict["error"] = error
        if run is not UNSET:
            field_dict["run"] = run
        if api_key is not UNSET:
            field_dict["apiKey"] = api_key
        if from_release is not UNSET:
            field_dict["fromRelease"] = from_release
        if to_release is not UNSET:
            field_dict["toRelease"] = to_release
        if class_ is not UNSET:
            field_dict["class"] = class_
        if reasons is not UNSET:
            field_dict["reasons"] = reasons
        if approved_by is not UNSET:
            field_dict["approvedBy"] = approved_by
        if auto is not UNSET:
            field_dict["auto"] = auto
        if undo is not UNSET:
            field_dict["undo"] = undo

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_1 import UpdateReasonType1
        from ..models.update_reason_type_6 import UpdateReasonType6
        from ..models.package_ref import PackageRef
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2

        d = dict(src_dict)
        from_ = d.pop("from")

        to = d.pop("to")

        at = d.pop("at")

        by = d.pop("by")

        ok = d.pop("ok")

        error = d.pop("error", UNSET)

        run = d.pop("run", UNSET)

        api_key = d.pop("apiKey", UNSET)

        _from_release = d.pop("fromRelease", UNSET)
        from_release: Union[Unset, PackageRef]
        if isinstance(_from_release, Unset):
            from_release = UNSET
        else:
            from_release = PackageRef.from_dict(_from_release)

        _to_release = d.pop("toRelease", UNSET)
        to_release: Union[Unset, PackageRef]
        if isinstance(_to_release, Unset):
            to_release = UNSET
        else:
            to_release = PackageRef.from_dict(_to_release)

        _class_ = d.pop("class", UNSET)
        class_: Union[Unset, UpgradeClass]
        if isinstance(_class_, Unset):
            class_ = UNSET
        else:
            class_ = UpgradeClass(_class_)

        reasons = []
        _reasons = d.pop("reasons", UNSET)
        for reasons_item_data in _reasons or []:

            def _parse_reasons_item(
                data: object,
            ) -> Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
                "UpdateReasonType6",
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_0 = (
                        UpdateReasonType0.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_1 = (
                        UpdateReasonType1.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_2 = (
                        UpdateReasonType2.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_3 = (
                        UpdateReasonType3.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_3
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_4 = (
                        UpdateReasonType4.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_4
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_5 = (
                        UpdateReasonType5.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_5
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_update_reason_type_6 = UpdateReasonType6.from_dict(
                    data
                )

                return componentsschemas_update_reason_type_6

            reasons_item = _parse_reasons_item(reasons_item_data)

            reasons.append(reasons_item)

        approved_by = d.pop("approvedBy", UNSET)

        auto = d.pop("auto", UNSET)

        undo = d.pop("undo", UNSET)

        upgrade = cls(
            from_=from_,
            to=to,
            at=at,
            by=by,
            ok=ok,
            error=error,
            run=run,
            api_key=api_key,
            from_release=from_release,
            to_release=to_release,
            class_=class_,
            reasons=reasons,
            approved_by=approved_by,
            auto=auto,
            undo=undo,
        )

        upgrade.additional_properties = d
        return upgrade

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
