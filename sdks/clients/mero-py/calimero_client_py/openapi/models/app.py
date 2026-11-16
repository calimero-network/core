from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_components import AppComponents
from ..models.app_executor import AppExecutor
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.app_restore import AppRestore
    from ..models.access_type_0 import AccessType0
    from ..models.access_type_3 import AccessType3
    from ..models.package_ref import PackageRef
    from ..models.access_type_1 import AccessType1
    from ..models.access_type_4 import AccessType4
    from ..models.access_type_2 import AccessType2
    from ..models.upgrade import Upgrade


T = TypeVar("T", bound="App")


@_attrs_define
class App:
    """An app instance (a context) as the directory records it. `executor` is absent for an SDK 0.12 app (wasmer), which
    this node lists but does not run; `release` is the package release it runs, absent for a record from before or a raw
    upload; `restore` is a restore from a backup under way.

        Attributes:
            app_id (str):
            name (str):
            group (int):
            bundle (str):
            owner (str):
            org (Union[None, str]):
            access (Union['AccessType0', 'AccessType1', 'AccessType2', 'AccessType3', 'AccessType4']): Who may reach an app
                at all.
            confirm_methods (list[str]):
            replicas (list[int]):
            created_at (int):
            init_args (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            metadata (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
            executor (Union[Unset, AppExecutor]):
            compute_bundle (Union[Unset, str]):
            components (Union[Unset, AppComponents]):
            last_upgrade (Union[Unset, Upgrade]): The last upgrade tried on an app, done or failed: `from` and `to` are
                bundle hashes, `by` the account (or `operator`), `run` the organization-wide run it was part of, `apiKey` the
                upgrade key that made it.
            deployment (Union[Unset, str]):
            release (Union[Unset, PackageRef]): One service of one package release: `service` is absent for a single-service
                bundle, `compute` (absent: false) marks the service's compute component.
            restore (Union[Unset, AppRestore]):
    """

    app_id: str
    name: str
    group: int
    bundle: str
    owner: str
    org: Union[None, str]
    access: Union[
        "AccessType0", "AccessType1", "AccessType2", "AccessType3", "AccessType4"
    ]
    confirm_methods: list[str]
    replicas: list[int]
    created_at: int
    init_args: Any
    metadata: Any
    executor: Union[Unset, AppExecutor] = UNSET
    compute_bundle: Union[Unset, str] = UNSET
    components: Union[Unset, AppComponents] = UNSET
    last_upgrade: Union[Unset, "Upgrade"] = UNSET
    deployment: Union[Unset, str] = UNSET
    release: Union[Unset, "PackageRef"] = UNSET
    restore: Union[Unset, "AppRestore"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_restore import AppRestore
        from ..models.access_type_0 import AccessType0
        from ..models.access_type_3 import AccessType3
        from ..models.package_ref import PackageRef
        from ..models.access_type_1 import AccessType1
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.upgrade import Upgrade

        app_id = self.app_id

        name = self.name

        group = self.group

        bundle = self.bundle

        owner = self.owner

        org: Union[None, str]
        org = self.org

        access: dict[str, Any]
        if isinstance(self.access, AccessType0):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType1):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType2):
            access = self.access.to_dict()
        elif isinstance(self.access, AccessType3):
            access = self.access.to_dict()
        else:
            access = self.access.to_dict()

        confirm_methods = self.confirm_methods

        replicas = self.replicas

        created_at = self.created_at

        init_args = self.init_args

        metadata = self.metadata

        executor: Union[Unset, str] = UNSET
        if not isinstance(self.executor, Unset):
            executor = self.executor.value

        compute_bundle = self.compute_bundle

        components: Union[Unset, str] = UNSET
        if not isinstance(self.components, Unset):
            components = self.components.value

        last_upgrade: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.last_upgrade, Unset):
            last_upgrade = self.last_upgrade.to_dict()

        deployment = self.deployment

        release: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.release, Unset):
            release = self.release.to_dict()

        restore: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.restore, Unset):
            restore = self.restore.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "appId": app_id,
                "name": name,
                "group": group,
                "bundle": bundle,
                "owner": owner,
                "org": org,
                "access": access,
                "confirmMethods": confirm_methods,
                "replicas": replicas,
                "createdAt": created_at,
                "initArgs": init_args,
                "metadata": metadata,
            }
        )
        if executor is not UNSET:
            field_dict["executor"] = executor
        if compute_bundle is not UNSET:
            field_dict["computeBundle"] = compute_bundle
        if components is not UNSET:
            field_dict["components"] = components
        if last_upgrade is not UNSET:
            field_dict["lastUpgrade"] = last_upgrade
        if deployment is not UNSET:
            field_dict["deployment"] = deployment
        if release is not UNSET:
            field_dict["release"] = release
        if restore is not UNSET:
            field_dict["restore"] = restore

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_restore import AppRestore
        from ..models.access_type_0 import AccessType0
        from ..models.access_type_3 import AccessType3
        from ..models.package_ref import PackageRef
        from ..models.access_type_1 import AccessType1
        from ..models.access_type_4 import AccessType4
        from ..models.access_type_2 import AccessType2
        from ..models.upgrade import Upgrade

        d = dict(src_dict)
        app_id = d.pop("appId")

        name = d.pop("name")

        group = d.pop("group")

        bundle = d.pop("bundle")

        owner = d.pop("owner")

        def _parse_org(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        org = _parse_org(d.pop("org"))

        def _parse_access(
            data: object,
        ) -> Union[
            "AccessType0", "AccessType1", "AccessType2", "AccessType3", "AccessType4"
        ]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_0 = AccessType0.from_dict(data)

                return componentsschemas_access_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_1 = AccessType1.from_dict(data)

                return componentsschemas_access_type_1
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_2 = AccessType2.from_dict(data)

                return componentsschemas_access_type_2
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_access_type_3 = AccessType3.from_dict(data)

                return componentsschemas_access_type_3
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_access_type_4 = AccessType4.from_dict(data)

            return componentsschemas_access_type_4

        access = _parse_access(d.pop("access"))

        confirm_methods = cast(list[str], d.pop("confirmMethods"))

        replicas = cast(list[int], d.pop("replicas"))

        created_at = d.pop("createdAt")

        init_args = d.pop("initArgs")

        metadata = d.pop("metadata")

        _executor = d.pop("executor", UNSET)
        executor: Union[Unset, AppExecutor]
        if isinstance(_executor, Unset):
            executor = UNSET
        else:
            executor = AppExecutor(_executor)

        compute_bundle = d.pop("computeBundle", UNSET)

        _components = d.pop("components", UNSET)
        components: Union[Unset, AppComponents]
        if isinstance(_components, Unset):
            components = UNSET
        else:
            components = AppComponents(_components)

        _last_upgrade = d.pop("lastUpgrade", UNSET)
        last_upgrade: Union[Unset, Upgrade]
        if isinstance(_last_upgrade, Unset):
            last_upgrade = UNSET
        else:
            last_upgrade = Upgrade.from_dict(_last_upgrade)

        deployment = d.pop("deployment", UNSET)

        _release = d.pop("release", UNSET)
        release: Union[Unset, PackageRef]
        if isinstance(_release, Unset):
            release = UNSET
        else:
            release = PackageRef.from_dict(_release)

        _restore = d.pop("restore", UNSET)
        restore: Union[Unset, AppRestore]
        if isinstance(_restore, Unset):
            restore = UNSET
        else:
            restore = AppRestore.from_dict(_restore)

        app = cls(
            app_id=app_id,
            name=name,
            group=group,
            bundle=bundle,
            owner=owner,
            org=org,
            access=access,
            confirm_methods=confirm_methods,
            replicas=replicas,
            created_at=created_at,
            init_args=init_args,
            metadata=metadata,
            executor=executor,
            compute_bundle=compute_bundle,
            components=components,
            last_upgrade=last_upgrade,
            deployment=deployment,
            release=release,
            restore=restore,
        )

        app.additional_properties = d
        return app

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
